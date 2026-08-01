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

No later phase may remove a boundary needed by an earlier phase. Performance
decisions compare only with the current rolling baseline; original revisions
remain non-blocking release history. Physical source moves begin only after
Desktop uses Reader.

### Execution Contract

- [ ] Keep routine implementation items fast: declare no more than five files,
  target no more than 200 changed lines, and run formatting, type, lint, and
  affected correctness tests. Add a single release smoke when the item changes
  startup, packaging, or a performance-sensitive path; routine items do not
  require paired performance capture.
- [ ] Before every migration or optimization checkpoint, record its type,
  primary metric, protected metrics, fixture, commands, and no more than five
  directly affected files. Mechanical source moves and this roadmap archive
  are the declared scope exceptions.
- [ ] Keep mechanical migration and performance optimization in separate
  candidates. Use Git renames for source moves and leave the workspace buildable
  after every accepted candidate.
- [ ] Run one bounded A-B-B-A performance gate only after a coherent change to
  a measured hot path. Select only the affected boundary, backend, mode, axis,
  or range; do not run a full matrix by default.
- [ ] Require ten internally calibrated samples of at least 25 ms in each
  timing or throughput capture. Stop the complete workload after 120 seconds,
  checkpoint after each child, and never add processes automatically.
- [ ] Accept preservation when protected combined regression is at most 3% and
  neither order pair regresses more than 5%. Accept optimization only when both
  order pairs improve, the combined primary reaches its declared target, and
  protected metrics stay within those limits.
- [ ] Treat noise above 2%, order-direction conflict, or a missing deciding
  metric as Inconclusive. A smaller non-regressing improvement is No-change;
  reliable regression or a hard-floor failure is Regression.
- [ ] Update the rolling baseline only after Pass. Keep original revisions and
  old stress results as non-blocking history.
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

- [x] Separate Acceptance, Benchmark, Performance gate, and Profile work.
- [x] Replace executable v1/v2 gates and stress matrices with schema v3,
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
- [x] Exit U1 only when configuration and Reader correctness pass and query,
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
- [ ] Pass the narrow Reader optimization gate against the rolling baseline;
  protect the incumbent full query within the 3% combined and 5% ordered-pair
  limits.
- [ ] Pass the compact 4 Run × 2 Metric dual-View RSS optimization gate with its
  declared target. Record original-revision trend only as history.
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
  unsupported-schema rejection, and source preservation; pass GPUI tests for Source
  confirmation, reload, autosave, archive/unimport, recovery, export, and
  import. Run `cargo check`, `cargo test`, and the protected Viewer gates.

### U7: Final Resource, Metal, and Release Qualification

U7 begins after U0–U6 pass. CI/release workflow changes remain a separate
review slice and require explicit approval under the repository boundary.

- [ ] Pass complete correctness and release Acceptance, then run only the
  reporting, Reader, Viewer CPU, and Viewer RSS gates affected since their
  rolling revisions.
- [ ] Use the compact dual-View workload for automated peak RSS: 4 Runs, 2
  Metrics, 100,000 points per series, three zoom round-trips, and four-way reads.
- [ ] Prove stale requests and cancellation retain no snapshot or query working
  set with ordinary small-fixture tests, not an RSS stress matrix.
- [ ] Run Instruments, Metal, allocation, or display Profiles only when a gate
  or acceptance failure needs diagnosis. Record them as non-blocking evidence.
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
