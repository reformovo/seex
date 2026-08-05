# Seex Roadmap

> This file contains only current and future work. Completed 0.1.x phases and
> their measured results are summarized as release history in
> [`release-notes/0.1.0b0.md`](release-notes/0.1.0b0.md#completed-01x-roadmap).
> Shipped release details and completed roadmap history live together in
> `release-notes/`.

Pre-1.0 releases do not promise store, API, or machine-output compatibility.
Cargo `0.1.0-beta.1` and Python `0.1.0b1` package identities are prepared from
one source. U7 creates tag `v0.1.0-beta.1` only after release qualification.

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
U0 performance contract -> U1 configuration + Reader -> U2 storage/query
         -> U3 reporting/Run SDK -> U4 Python API
         -> U5 crate/package convergence
         -> U6 Viewer configuration/workbench -> U7 release qualification
```

No later phase may remove a boundary needed by an earlier phase. Performance
comparisons use only the current rolling baseline and never block a phase;
original revisions remain release history. Physical source moves begin only
after Desktop uses Reader.

### Execution Contract

- [ ] Keep routine implementation items fast: declare no more than five files,
  target no more than 200 changed lines, and run formatting, type, lint, and
  affected correctness tests. Add a single release smoke when the item changes
  startup, packaging, or a performance-sensitive path; routine items do not
  require a performance comparison.
- [ ] Before every migration or optimization checkpoint, record its type,
  primary metric, protected metrics, fixture, commands, and no more than five
  directly affected files. Mechanical source moves and this roadmap archive
  are the declared scope exceptions.
- [ ] Keep mechanical migration and performance optimization in separate
  candidates. Use Git renames for source moves and leave the workspace buildable
  after every accepted candidate.
- [ ] Run one bounded A-B-B-A performance comparison only after a coherent
  change to a measured hot path. Select only the affected boundary, backend,
  mode, axis, or range; do not run a full matrix by default.
- [ ] Require ten internally calibrated samples of at least 25 ms in each
  timing or throughput capture. Stop the complete workload after 120 seconds,
  checkpoint after each child, and never add processes automatically.
- [ ] Classify preservation as Pass when protected combined regression is at
  most 3% and neither order pair regresses more than 5%. Classify optimization
  as Pass only when both order pairs improve, the combined primary reaches its
  declared target, and protected metrics stay within those limits.
- [ ] Treat noise above 2%, order-direction conflict, or a missing deciding
  metric as Inconclusive. A smaller non-regressing improvement is No-change;
  reliable regression or a hard-floor failure is Regression.
- [ ] Update the rolling baseline only after Pass. No-change, Regression, and
  Inconclusive remain non-blocking observations. Keep original revisions and
  old stress results as history.
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

### U0: Lightweight Performance Contract

- [x] Separate Acceptance, Benchmark, Performance comparison, and Profile work.
- [x] Replace executable v1/v2 comparisons and stress matrices with schema v3,
  calibrated raw samples, one A-B-B-A sequence, atomic checkpoints, and a
  120-second workload budget.
- [x] Keep historical U0/U1 measurements and source workloads under
  `docs/reference/`; they do not decide new candidates.
- [x] Freeze scoped v3 rolling identities without running unrelated boundaries.
  Admission and Reader self-comparisons passed; durability, chart CPU, and
  compact dual-View RSS returned Inconclusive under the fixed 2% noise policy
  and were recorded without reruns or expanded workloads.

### U1: Configuration Foundation, Public `seex` Facade, and Reader First

U1 establishes the configuration and identity contract before Reader migration.
Configuration, document codecs, the facade, and Reader migration remain
separate candidates. The configuration work does not change public Rust or
Python APIs; the facade and Reader add only the already-planned Rust surface,
and the shipped Python API remains unchanged until U4.

#### U1.1: Configuration and document foundation

- [x] Define schema version 1 for global `~/.seex/config.toml` and project
  `<root>/.seex/config.toml`. Resolve explicit SDK arguments, project config,
  global config, and built-in defaults in that order; merge S3 tables field by
  field and ignore inherited credentials when effective `data_path` is local.
- [x] Keep field-level ownership explicit: the SDK reads storage and S3 keys;
  Desktop reads and edits Sources. Share contract fixtures without exposing
  Desktop configuration types through the public SDK.
- [x] Introduce stable Source aliases and represent Viewer Project and Run
  references by alias rather than path. Validate alias conflicts and Source
  Project allowlists, and load Runs only for allowed Projects.
- [x] Implement schema version 1 TOML workbench encoding, decoding, and
  validation. Reject malformed and unsupported documents without migration.
- [x] Use the approved direct `toml_edit` dependency for Desktop
  read-modify-write. Preserve comments, unknown fields, native storage fields,
  and secrets; require owner-only permissions for global secrets; write through
  a same-directory temporary file and reject a stale read fingerprint.
- [x] Prove SDK `init`, `log`, `finish`, and `shutdown` may read effective
  configuration but never rewrite either config or workbench file.

#### U1.2: Facade and Reader migration

- [x] Create unpublished `crates/seex` as a facade over the current
  `seex-model`, `seex-storage`, and `seex-core` crates. Keep every workspace
  target buildable.
- [x] Define public `Reader`/`ReaderBuilder`, `MetricAxis`, typed half-open
  ranges, strict caller-selected `max_points`, and `MetricSeries`.
- [x] Keep pixels out of public Rust and Python queries. Desktop alone converts
  a closed viewport into crate-private options for one real neighbor on each
  side, without weakening the public point bound.
- [x] Make `MetricSeries` retain real samples, source count, downsampled state,
  completeness, and reasons, and expose an Arrow PyCapsule stream directly.
- [x] Keep `ProjectConnection`, `ProjectMetricReader`, `NativeQueryStore`,
  storage errors, DuckDB types, and local-only source policy private.
- [x] Migrate Desktop discovery and curve reads to Reader over configured
  Source aliases and Project allowlists. Preserve four-way scheduling,
  generation reconciliation, hover/locked-cursor real-sample semantics, and
  source-specific failures.
- [x] Route PyO3 through a temporary compatibility adapter without changing the
  shipped Python surface; defer the breaking public API switch to U4.
- [x] Cover configuration layering, path bases, schema and alias conflicts,
  allowlists, TOML round trips, source preservation, concurrent edits, and
  byte-for-byte SDK non-mutation, plus Reader/native/standalone parity, all
  axes and range types, strict bounds, Desktop neighbors, missing metadata,
  and Arrow output.
- [x] Exit U1 when configuration and Reader Acceptance checks pass. Record
  affected query, reporting, Viewer CPU, and RSS comparisons against the rolling
  baseline without making their classifications blocking.

### U2: Storage Migration and Query Peak Reduction

Profiling identifies DuckDB window/sort materialization and allocator churn as
the remaining peak-memory path. Full-span and narrow queries therefore use
different execution plans; one universal SQL plan is not a goal.

#### U2.1: Mechanical storage move

- [x] Move model, storage, and query implementation into private
  `seex::{model, storage}` modules with Git renames. Leave the old unpublished
  crates as re-exports until all consumers migrate.
- [x] Run the migration comparison without SQL, reduction, allocation, schema,
  or behavior changes. The scoped Reader preservation comparison classified
  Pass: narrow changed -0.14% and full changed -0.64%, with no ordered-pair
  regression above 1%.

#### U2.2: Split query plans

- [x] Preserve the incumbent Overview/full-span SQL and its materialized
  ordered plan. Record its reliable latency and RSS as protected metrics with
  the 3% comparison threshold.
- [x] Add a narrow Step plan that filters the viewport before expensive
  materialization and bucket windows while retaining every same-step
  replacement until last-write-wins.
- [x] Return one real effective neighbor on each side and reduce only the
  cropped effective rows. Preserve duplicates, spikes, strict point budgets,
  evidence reasons, and DuckDB/SQLite parity.
- [x] Initially keep relative-time and timestamp queries on the incumbent plan;
  a replacement may change its timestamp, so time filtering cannot precede
  last-write-wins without a separate proof.

The scoped Reader comparison observed a 28.93% combined narrow improvement,
while full changed by 1.18% with no ordered-pair regression above 3.02%. The
result is Inconclusive because the protected full metric had an A-B/B-A
direction conflict. It was not rerun, and the rolling baseline remains
`9edd1cd`.

#### U2.3: Diagnostics and cancellation

- [x] Separate whole-series negative/decreasing/completeness diagnostics from
  viewport selection. Finished Runs cache by source/project/run/metric;
  Running Runs invalidate on refresh or storage-generation change.
- [x] Propagate diagnostics failure as incomplete evidence with an explicit
  reason. The scoped Reader checkpoint passed with narrow -0.43% and full
  -0.49%; failures retain available points as Partial with
  `DiagnosticsUnavailable`.
- [x] Give every cloned DuckDB connection an interrupt handle and request token.
  A superseded generation interrupts only its current request; interruption is
  stale cancellation and never becomes an error snapshot.
- [x] Check supersession before query execution and between Runs. Stale results
  must not enter a merged snapshot or retain their query working set.

#### U2.4: Evidence-driven fallbacks

- [x] When narrow Step SQL did not reach the compact RSS target, profile and test
  a separate narrow-only bounded reducer. Its existing Query captures
  reclassify as Pass under the corrected primary-only direction rule, but its
  compact Viewer RSS comparison improved only 12.5%, missed the 25% target,
  and was Inconclusive above the 2% noise limit. The candidate was reverted.
- [x] After the bounded reducer missed the RSS target, profile Parquet
  physical ordering and row-group sizing as the next independent candidate.
  The fixture has Step-range row-group statistics and the narrow scan receives
  a Step dynamic filter, so no physical-layout candidate was opened.
- [x] Do not retry generic `HASH_GROUP_BY` extrema, memory limits, DuckDB thread
  caps, allocator relief scheduling, or removal of the beneficial full-span
  ordered window without new contradictory profile evidence.

#### U2 exit evidence

- [x] Pass full/narrow correctness, DuckDB/SQLite parity, last-write-wins,
  neighbors, diagnostics, and nearest-real-sample hover tests.
- [x] Record the narrow Reader optimization comparison against the rolling
  baseline, including the incumbent full query as a protected metric.
- [x] Record the compact 4 Run × 2 Metric dual-View RSS comparison and retain
  original-revision trends only as history.
- [x] Prove superseded queries merge no snapshot and retain no working set.

Ordinary small-fixture tests cover cancellation before execution, between Runs,
before merge, connection-bound interruption, unrelated request keys, old-token
completion, stale-event suppression, zero retained stale snapshots, and release
of both the active registry entry and outstanding request ticket.

The compact Viewer comparison observed a 10.2% combined peak-RSS improvement,
but peak and warm RSS exceeded the 2% cross-process noise limit. It is
Inconclusive, was not rerun, and does not advance the rolling baseline. The
rolling baseline remains `9edd1cd`. Together with completed Acceptance and
fallback investigation, this closes U2 and allows U3 to begin. See
[`reference/u2-performance-observations.md`](reference/u2-performance-observations.md).

### U3: Engine Migration and Rust Run SDK

Mechanical engine movement and reporting behavior changes are separate
candidates.

- [x] Move lifecycle, queue, writer, diagnostics, comparison, and ranking into
  private `seex::engine`; keep `seex-core` as an unpublished re-export until
  consumers migrate. Record a scoped migration comparison using the 3%
  preservation threshold.
- [x] Implement public `Client`/`ClientBuilder`, `RunHandle`, `RunOptions`,
  `LogOptions`, `ResumePolicy`, and matchable `Error`/`Result`.
- [x] Make `RunHandle: Clone + Send + Sync`; clones share one admission lock,
  step cursor, queue, and terminal state.
- [x] Admit a non-empty Mapping atomically with at most 8,192 numeric metrics.
  Failure admits no subset and does not advance the cursor; queue capacity and
  diagnostics count points consistently.
- [x] Default explicit-step `commit` to false and implicit-step `commit` to
  true. Start a new Run at step zero, resume from the greatest persisted step,
  and reject committed-step regression.
- [x] Make matching terminal operations retry incomplete drain/flush work.
  Return a typed error for a conflicting terminal outcome and never hide a
  finalization error in `Drop`.
- [x] Cover Project get-or-create races, resume modes, cloned-handle races,
  atomic queue failure, cursor/commit cases, finalization barriers, and
  persistence without lost reports or partial Mappings.
- [x] Observe explicit single-metric admission against the 100,000 calls/s
  target. Across five runs, compare implicit single-metric median throughput
  with the 90% explicit-throughput target; count multi-metric throughput per
  point.
- [x] Exit U3 when engine and reporting Acceptance checks pass. Record scoped
  p50/p95, drain latency, persistence, and peak RSS comparisons without making
  their classifications blocking.

The mechanical migration passed workspace Acceptance. Its original combined
admission/durability workload produced no capture before the 120-second budget;
the narrowed durability A-B-B-A completed but was Inconclusive because the
strict parser could not see the test-harness-prefixed drain record. It was not
rerun, and no reporting baseline advanced.

The final public Run SDK benchmark at `e4955fe` observed 4.00 million explicit
calls/s p50, a 106.6% implicit/explicit median ratio, 8.67 million Mapping
points/s p50, 143.640 ms drain p50, 39.189 ms finalization p50, and 264.8 MB
peak RSS p50. The records and reporting RSS A-B-B-A comparisons were both
Inconclusive under the 2% noise rule, so neither non-blocking classification
advanced a rolling baseline. See
[`u3-performance-observations.md`](reference/u3-performance-observations.md).

### U4: Python Run, Api, CLI, and Arrow Surface

U4 is the beta API reset. It does not retain the shipped public Client
as a compatibility layer.

- [x] Implement typed `seex.init(...) -> seex.Run` with the accepted
  project/id/name/resume/settings semantics and secret-redacted configuration.
- [x] Implement Mapping `Run.log`, context management, `finish(exit_code)`, and
  advanced diagnostics over the Rust Run SDK. Preserve an original context
  exception and attach finalization failure as context.
- [x] Add read-only `seex.Api`, `RunRecord.history()`, metrics, summary,
  comparison, and ranking over Reader. Start no writer for read-only use.
- [x] Limit `history()` axes to step, relative time, and timestamp. Require
  matching typed bounds and a caller-selected strict `max_points`; reject
  coercion and arbitrary metric x-axis joins.
- [x] Make Python `MetricSeries` implement Arrow PyCapsule streaming directly.
  Remove separate public table-query methods after every consumer migrates.
- [x] Move the CLI to public Rust/Python facades and remove calls to private
  underscore PyO3 APIs. Preserve deterministic versioned JSON contracts.
- [x] Update Python type stubs and cover init/resume, Mapping validation,
  context outcomes, Api discovery, range errors, evidence, Arrow, Reader parity,
  CLI JSON, and packaging smoke tests.
- [x] Pass Python formatting/lint, Pyright, pytest, Rust/PyO3 parity, wheel and
  sdist smoke tests, then record affected reporting/query migration comparisons.

U4 passed correctness and isolated wheel/sdist Acceptance at revision
`19f00aba232557dc71a155c3c137ff3bb3281df6`. Reporting admission was a
non-blocking Regression and durability was Inconclusive, so neither reporting
baseline advanced. DuckDB Reader Step preservation passed; its rolling
baseline advanced to the candidate revision. See
[`python-api-performance-observations.md`](reference/python-api-performance-observations.md).

### U5: Single-Crate Convergence and Packaging

- [x] After every consumer migrates, delete the temporary `seex-model`,
  `seex-storage`, and `seex-core` crates and re-exports.
- [x] Mechanically rename `seex-chart-core` to unpublished `seex-plot` without
  behavior or performance changes. Keep it free of SDK, storage, PyO3, and GPUI
  dependencies.
- [x] Make `seex` the only publishable workspace crate. Set `publish = false`
  for `seex-python`, `seex-app`, and `seex-plot`.
- [x] Verify `cargo package -p seex` contains no path dependency or PyO3, GPUI,
  Desktop, or plot source; unpack and build/test it in an independent directory.
- [x] Align Cargo `0.1.0-beta.1` and Python `0.1.0b1` to one source without
  changing catalog/Parquet schemas or adding a runtime dependency. The matching
  `v0.1.0-beta.1` tag is deliberately deferred to U7.
- [x] Run warning-free Rust formatting, Clippy, check, tests, doc tests, docs,
  package verification, Python Acceptance, and affected performance comparisons.

U5 converged the workspace at revision `0fab2af41727928083de6faa24f10f022378a2f5`.
Local, package, wheel/sdist, CLI/Arrow, and online/offline LTTB Acceptance passed.
MinIO/S3 and live partition-pruning were unavailable because the four required
`SEEX_MINIO_*` connection variables were unset; `mc` was available. The compact
dual-View RSS comparison was Inconclusive and did not advance its rolling
baseline. See [`u5-performance-observations.md`](reference/u5-performance-observations.md).

### U6: Viewer Configuration and Workbench Experience

U6 completes the accepted Viewer configuration and workbench design after
crate convergence and before final release qualification. Keep Source
management, autosave, and export/import as separate candidates.

#### U6.1: Source management and scope

- [x] Implement Source directory preflight, editable alias and Project
  multi-selection confirmation, configuration writeback, Manage Projects, and
  Reload Sources. Reject an invalid reload without replacing the last valid
  live Sources.
- [x] Select project scope only from the Source root used to launch or
  explicitly open Viewer. Importing another Source does not change scope;
  configuration merges global and project documents, while workbenches never
  merge and no project scope uses `~/.seex/workbench.toml`.
- [x] Keep Archive Project as reversible workbench state. Make Remove Project
  a confirmed unimport that removes the allowlist entry and its workbench
  references; retire persisted `removed_projects`.

#### U6.2: Semantic autosave

- [x] Persist Views and the active View; selected, baseline, pinned, and
  archived Runs and Projects; Metrics, selected Metric, row heights, axis and
  viewport; major component visibility and dimensions; and expanded Projects.
- [x] Exclude Source paths and allowlists, hover/focus and menu state, filter
  text, scroll positions, query results, pending tasks, cursors, and transient
  errors from workbench state.
- [x] Have `WorkbenchSession` produce immutable semantic snapshots and
  coalesce changes. Serialize and atomically replace the file on a GPUI
  background task, then update entities on the foreground; render callbacks
  consume plain snapshots and never perform I/O or re-enter an Entity.
- [x] Report invalid or unsupported documents without overwriting them. Retain
  unavailable Source, Project, and Run references across save and restart so
  they recover when mappings or data return.

#### U6.3: Export, import, and exit criteria

- [x] Export only `workbench.toml`, never config, Source paths, allowlists,
  native data, secrets, or transient state.
- [x] Before import, autosave the current workbench and preflight every alias,
  Project, rewrite, and allowlist addition. After confirmation, complete all
  file writes before switching live configuration and workbench state; a
  failure keeps the previous live state.
- [x] Cover every acceptance scenario in the accepted design, including
  owner-only secret files, invalid external edit fallback, alias remapping,
  missing-reference recovery, archive/unimport behavior, and failed import
  without a live-state switch.
- [x] Pass pure Rust tests for configuration, permissions, aliases, codecs,
  unsupported-schema rejection, and source preservation; pass GPUI tests for Source
  confirmation, reload, autosave, archive/unimport, recovery, export, and
  import. Run `cargo check`, `cargo test`, and affected Viewer comparisons.

U6 completed the Viewer configuration and workbench experience at candidate
revision `12f6a555d0b5ad4a9891ffaf3de2efcb7e740f26`. All local Rust, GPUI,
release startup, and Python Acceptance passed. The compact dual-View RSS
preservation comparison passed and advanced its rolling baseline; the chart
CPU comparison was not run because U6 did not modify the `seex-plot` chart CPU
path. See
[`u6-performance-observations.md`](reference/u6-performance-observations.md).

### U7: Final Resource, Metal, and Release Qualification

U7 begins after U0–U6 pass. CI/release workflow changes remain a separate
review slice and require explicit approval under the repository boundary.

- [x] Pass complete correctness and release Acceptance, then run only the
  reporting, Reader, Viewer CPU, and Viewer RSS comparisons affected since their
  rolling revisions.
- [ ] After release Acceptance passes, create `v0.1.0-beta.1` from the exact
  source whose Cargo and Python packages identify as beta.1; do not retag a
  different source.
- [x] Use the compact dual-View workload for automated peak RSS: 4 Runs, 2
  Metrics, 100,000 points per series, three zoom round-trips, and four-way reads.
- [x] Prove stale requests and cancellation retain no snapshot or query working
  set with ordinary small-fixture tests, not an RSS stress matrix.
- [x] Run Instruments, Metal, allocation, or display Profiles only when a
  comparison or Acceptance failure needs diagnosis. Record them as
  non-blocking evidence.
- [x] Only after every release Acceptance check passes, add a separately
  reviewed macOS ARM64 CI job that verifies the Xcode Metal Toolchain and builds
  the unsigned Viewer without changing the Python wheel matrix.
- [x] On matching tags, produce `seex-app-macos-aarch64`, SHA-256 checksum, and
  attestations, while proving Desktop artifacts cannot publish to PyPI.
- [x] Publish crates.io before PyPI. If the `seex` crate name cannot be claimed,
  stop and revisit the naming decision rather than silently choosing a fallback.

### V1: Viewer Run-Elapsed-Time Alignment

The Viewer currently presents `AlignmentAxis::ElapsedTime` as an absolute
observation timestamp. That makes sequential Runs occupy disjoint wall-clock
ranges and conflicts with the renderer-independent comparison contract. V1
restores Run-relative alignment while retaining the observation timestamp as a
stored fact and typed Reader axis. The order is fixed, and every checklist item
is a separate reviewable commit within the routine five-file and 200-line scope.

- [x] **V1.1 — Curve semantics and ruler.** Map the Viewer time axis to
  `MetricAxis::RelativeTime`, query half-open relative-time ranges, and project
  `MetricCoordinate::RelativeTime`. Rename the Viewer curve axis and ruler to
  Elapsed time, format it as a non-wrapping duration, and prove overview,
  detail, and GPUI axis-picker behavior without changing Step behavior.
- [ ] **V1.2 — Axis-switch interaction.** Rename the native application menu
  action to Elapsed Time and clear hover and locked cursors when the comparison
  axis changes so coordinates from one axis are never interpreted on another.
- [ ] **V1.3 — Workbench migration.** Encode new workbench state with an
  explicit `elapsed_time` axis. Read schema-v1 `timestamp` state as Elapsed time
  while discarding its incompatible epoch viewport; preserve schema-v1 Step
  viewports and cover migration plus current-schema round trips.
- [ ] **V1.4 — Product language.** Define Run elapsed time and observation
  timestamp in the project context, and update the workbench draft so the
  Viewer consumes the Core comparison semantics rather than inventing an
  Absolute time alignment axis. No new ADR is required.
- [ ] **V1 exit.** Preserve the Parquet schema, native storage boundary, public
  Rust/Python timestamp query surface, four-way Viewer reads, point budgets,
  and missing-Run-start evidence. Pass Rust formatting, Clippy, check, and
  workspace tests. This changes no measured algorithm or hot path, so it does
  not require a performance comparison.

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
