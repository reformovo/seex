# Seex Glossary

This glossary defines the native product language. Version-specific
architecture documents define which terms are contractually required for each
release.

## Product Terms
- **Seex**: The unified SDK, CLI, and desktop application brand, pronounced
  “six” (`/sɪks/`).
- **Project**: Lightweight namespace for related runs; metadata stays minimal.
- **Run**: One training execution with user-supplied or generated `run_id`.
- **Terminal run**: A run whose lifecycle state is `finished` or `failed`.
  Terminal runs are queryable, but they cannot be resumed for metric reporting.
- **Run finalization**: The explicit transition that closes metric admission
  for one run, drains reports admitted before the close barrier, writes a
  terminal lifecycle state, and then attempts terminal-run Parquet visibility.
- **Metric key**: User-facing metric name; path escaping is storage detail.
- **Metric series**: All points for one `(run_id, metric_key)` pair.
- **Metric point**: One numeric observation in a metric series.
- **Metric reporting**: The hot-path handoff from training code to Seex.
  Reporting must not block training progress.
- **Queued report**: A metric report received by the hot-path API but not yet
  durably admitted. Queued reports may be lost if the process exits before
  admission.
- **Metric queue**: The bounded in-process handoff used by the hot-path metric
  reporting API. A full metric queue is an admission failure, not a silent drop.
- **Accepted report**: A metric report that Seex has durably admitted and
  can recover after process restart. Admission to an in-process buffer alone is
  not acceptance.
- **Persisted metric point**: A metric point that has been written to the
  native storage engine and is visible to Seex queries under effective-series
  semantics.
- **Data discovery**: Traversal from stored projects to runs, metric series,
  and persisted metric points without requiring their identifiers in advance.
- **Read surface**: The read-only product boundary through which trainers and
  agents discover and consume stored Seex data. It is not an agent-specific
  API or direct access to storage internals.
- **Native project store**: The authoritative local collection of project
  metadata, run lifecycle state, and persisted metric points, including points
  not yet exported to Parquet.
- **Parquet dataset**: An open, fact-only representation of flushed metric
  points that follows the Seex Parquet compatibility contract. It is not a
  catalog or an authoritative native project store.
- **Run summary**: Derived per-run values for run lists and comparisons.
- **Comparison axis**: Requested basis for ordering metric observations across
  Runs without changing their measured values.
- **Objective metric**: Primary metric and direction used to compare Runs.
- **Comparison evidence**: Derived facts and qualifications supporting a Run
  comparison.
- **Comparison report**: One candidate/reference primary comparison plus
  ordered secondary metric evidence.
- **Secondary metric**: Supporting comparison evidence that does not affect
  objective outcome, preference, ranking, or tie-breaking.
- **Completeness**: Whether comparison evidence is complete, partial,
  unavailable, or invalid.
- **Outcome**: Numeric relationship between a candidate and its reference.
- **Preference**: Read-only advice derived from an outcome and its evidence.
- **Candidate**: Request role for the Run being evaluated.
- **Reference**: Request role for the Run against which a candidate is compared.
- **Baseline**: Explicitly requested reference role in a generic comparison.
- **Incumbent**: Reference role in an autoresearch comparison.
- **Metric aggregate**: Materialized-view-like index/state from metric writes.
- **Reporting diagnostics**: Minimal observable state for pending reports,
  queue-full failures, persisted reports, writer state, and last errors.
  Diagnostics are a runtime snapshot, not durable history, telemetry, or query
  results.
- **Flush diagnostics**: Runtime-only client diagnostics for the current
  process's latest terminal-run data flush attempt. They are not catalog state
  and are not recoverable after process restart.
- **Run-writer lock**: Local OS advisory lock that allows only one active
  writer client to hold a writable handle for a run. Lock files are runtime
  state, not catalog tables.

## Storage Terms
- **Seex logical schema**: Product-owned project, run, metric, point, and
  summary schema.
- **Parquet schema**: The open compatibility boundary for native metric data.
- **Catalog backend**: The database engine DuckLake uses for metadata in native
  mode. DuckDB is the default backend; SQLite is also supported locally.
- **Data path**: The location where DuckLake writes Parquet data files; local
  filesystem by default, S3-compatible object storage when configured.
- **Catalog path**: The local path or connection target used for DuckLake
  catalog metadata.
- **DuckLake**: The required native storage engine. It is a core
  dependency during validation, not a public product protocol.
- **Native mode**: Local-first operation without a separate service process.

## Out Of Current Native Scope
- Workspace and organization hierarchy.
- Cloud implementation and Cloud source files.
- Run or metric deletion.
- Agent memory tables such as hypothesis, insight, decision, and report.
- MCP server and tool-calling APIs.
