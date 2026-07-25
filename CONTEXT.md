# PulseOn Context

PulseOn tracks local-first training runs and numeric metric series. This
context defines the product language used across native architecture decisions.

## Language

**Project**:
A lightweight namespace for related training runs.

**Data source**:
An imported reference that makes one local native project store available to
the analysis workbench. One data source may contain multiple Projects.

**Analysis View**:
A named analysis workspace with its own selected Runs, metric panels, alignment,
and horizontal viewport.

**View baseline**:
An optional single Run chosen as the comparison reference for an Analysis View.
Choosing it does not change the Run's native data or lifecycle.

**Pinned Run**:
A Run placed exclusively in one Analysis View's viewer-only Pinned group instead
of that View's normal Project listing.

**Archived Run**:
A Run placed in the workbench-wide viewer-only Archived group and hidden from
normal Project listings in every Analysis View, without changing its native
lifecycle.
_Avoid_: Achieved Run, deleted Run

**Pinned Project**:
An imported Project placed in a workbench-only pinned Project location. This
does not pin, select, or otherwise change any Run in that Project.

**Archived Project**:
An imported Project hidden from the normal Project listing by the workbench,
without changing the Project or any of its Runs in native storage.
_Avoid_: Achieved Project, deleted Project

**Metric panel**:
One aligned chart track for a metric key across the Runs selected in an
Analysis View.

**Run**:
One training execution with a user-supplied or generated run identifier.
_Avoid_: Experiment

**Metric key**:
The user-facing name for a metric series.

**Metric series**:
All metric points for one run and metric key.

**Metric point**:
One numeric observation in a metric series.

**Metric reporting**:
The hot-path handoff from training code to PulseOn.

**Queued report**:
A metric report received by PulseOn but not yet durably admitted.

**Accepted report**:
A metric report that PulseOn has durably admitted and can recover after process
restart.

**Persisted metric point**:
A metric point that has been written to native storage and is visible to
PulseOn queries.

**Data discovery**:
The traversal from stored projects to runs, metric series, and persisted metric
points without requiring their identifiers in advance.

**Read surface**:
The read-only product boundary through which trainers and agents discover and
consume stored PulseOn data.
_Avoid_: Agent API, storage API

**Native project store**:
The authoritative local collection of project metadata, run lifecycle state,
and persisted metric points, including points not yet exported to Parquet.
_Avoid_: Parquet directory, viewer database

**Parquet dataset**:
An open, fact-only representation of flushed metric points that follows the
PulseOn Parquet compatibility contract.
_Avoid_: Native project store, catalog

**Closed run**:
A run that no longer accepts metric reports because it is being finalized or has
already reached a terminal state.

**Terminal run**:
A run whose lifecycle has ended as either finished or failed.

**Run finalization**:
The explicit transition of a run from running to a terminal lifecycle state.

**Metric aggregate**:
Derived index state over an effective metric series.

**Metric summary**:
The effective count, last point, minimum, and maximum for one complete effective
metric series.
_Avoid_: Viewport statistics

**Objective evidence**:
The last effective value of an objective metric for one Run, qualified by Run
status, completeness, and structured reasons.

**Ranking**:
A direction-aware competition ordering of an explicit pool of Runs using
eligible Objective evidence within one Project.
_Avoid_: Cross-Project leaderboard

**Catalog backend**:
The database engine used for native storage metadata.

**Catalog application table**:
A PulseOn-owned catalog table for control-plane or query-index state.
_Avoid_: DuckLake logical table, DuckLake internal table

**Data path**:
The local filesystem or S3-compatible location used for Parquet metric data.

**Metric key encoded**:
A storage-facing percent-encoded form of a metric key used for data-path
partitioning.
_Avoid_: Metric key partition

**Run-writer lock**:
A local OS advisory lock that allows only one active writer client to hold a
writable handle for a run.
_Avoid_: Lock table, lease, heartbeat, stale-lock cleanup service
