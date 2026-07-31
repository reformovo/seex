# Viewer Configuration and Workbench State

> Status: Draft

## Summary

Seex Viewer should separate durable configuration from semantic UI state.
Both are app-owned, versioned TOML documents, but they have different
lifecycles and portability rules:

- `config.toml` records machine-local Source configuration. It is editable by
  users and by Source-management UI.
- `workbench.toml` records the current workbench and is used for local autosave
  and explicit export/import.

The first audience is one user restoring a workbench across personal devices.
Transfer is explicit; this proposal does not introduce accounts, cloud sync,
shared-file synchronization, or team collaboration.

## Ownership and Boundaries

Viewer files have global and project scopes:

```text
global:  ~/.seex/config.toml          ~/.seex/workbench.toml
project: <root>/.seex/config.toml     <root>/.seex/workbench.toml
```

Project scope is selected by the Source root used to launch or explicitly open
the Viewer, not by a selected catalog Project. Importing another Source does
not change scope. Configuration merges global defaults with project overrides;
different definitions of the same Source alias are an error. Relative paths
resolve from `$HOME` when defined globally and `<root>` when defined by a
project. Workbenches never merge: project scope uses or creates its project
file, while no project scope uses the global file.

The project config is also the existing native storage config. It is one Seex
configuration with field-level ownership: the SDK reads storage and S3 keys,
while the desktop manages Sources and future settings. Storage keys and secrets
must not be copied into workbench state. Global storage and S3 values provide
defaults; project values override them, and S3 tables merge field by field.

The proposal changes no Rust or Python SDK API, catalog schema, Parquet schema,
or native storage responsibility. UI state remains owned by `seex-app`; it is
not stored in DuckDB, SQLite, DuckLake, or the Parquet data area.

SDK reporting may read effective configuration but `init`, `log`, `finish`, and
`shutdown` never rewrite config or read or write either workbench file.

## Application Configuration

`config.toml` has an explicit schema version and a table of stable Source
aliases. Each Source contains its machine-local path and the Project IDs that
the user has chosen to import. Version 1 reserves room for future settings but
does not define setting keys before a real preference is introduced.

Resolution order is explicit SDK argument, project config, global config, then
built-in default. The examples show every currently supported native config key
together with the proposed Source table; values are illustrative:

```toml
# ~/.seex/config.toml
schema_version = 1
catalog_backend = "duckdb"

[s3]
endpoint = "objects.example.com"
access_key_id = "example-access-key"
secret_access_key = "example-secret"
session_token = "example-session-token"
region = "us-east-1"
path_style = false
use_ssl = true

[sources.research]
path = "/Users/me/experiments"
projects = ["vision-baseline", "vision-large"]
```

```toml
# <root>/.seex/config.toml
schema_version = 1
catalog_path = ".seex/catalog.ducklake"
data_path = "s3://bucket/project-a"

[sources.local-benchmarks]
path = "/Volumes/data/seex"
projects = ["reader-benchmarks"]
```

`catalog_backend` accepts `duckdb` or `sqlite` and defaults to `duckdb`.
`catalog_path` is local-only; `data_path` accepts a local path or `s3://` URI.
The `[s3]` table is used only for S3 data, so inherited credentials are ignored
when a project selects a local `data_path`. Credentials stay connection-local,
and a global config containing them must be readable and writable only by its
owner. Desktop writes must preserve all native fields and secrets.

An alias is suggested from the selected directory name as a lowercase portable
identifier. The user may edit it before confirming import. It must be unique in
the configuration and does not change when the Source path changes.

Source import follows this sequence:

1. The user chooses a Source directory.
2. Seex reads Project summaries without importing every Project's Runs.
3. A confirmation view shows an editable alias and a Project multi-selection.
4. Confirmation atomically writes the Source and its non-empty Project
   allowlist to `config.toml`.
5. Seex loads Runs only for the selected Projects.

Manage Projects can add or remove allowlist entries later. Archive Project
remains a reversible workbench-state operation. Remove Project becomes an
unimport operation: after confirmation it removes the Project from the
allowlist and clears its workbench references. The separate persisted
`removed_projects` concept is retired.

The application loads configuration at startup. UI changes apply immediately.
External edits take effect only through Reload Sources. A reload validates the
whole candidate before changing live Sources; an invalid candidate leaves the
last valid configuration active.

App-originated updates use a syntax-preserving, atomic read-modify-write. They
preserve comments and unrelated or unknown TOML content. If the file changed
since it was read, Seex refuses to overwrite it and asks the user to reload
before retrying.

## Workbench State

`workbench.toml` contains semantic state rather than Source configuration. The
following example is representative, not an exhaustive field specification:

```toml
schema_version = 1
active_view = 0

[layout]
project_sidebar_visible = true
project_sidebar_width = 240.0
metric_sidebar_compact = false
bottom_inspector_visible = true
bottom_inspector_height = 220.0

[[expanded_projects]]
source = "research"
project_id = "vision-baseline"

[[views]]
name = "Training"
axis = "step"
selected_metric = "loss"
metrics = ["loss", "accuracy"]
viewport = [1000.0, 5000.0]

[[views.runs]]
source = "research"
project_id = "vision-baseline"
run_id = "run-2026-07-31"
```

Every Project reference uses `(source_alias, project_id)` and every Run
reference uses `(source_alias, project_id, run_id)`. Paths and Project
allowlists never appear in workbench state.

The workbench persists:

- Views and the active View;
- selected, baseline, pinned, and archived Runs and Projects;
- Metrics, selected Metric, row heights, axis, and viewport;
- major component visibility and dimensions; and
- expanded Projects.

It does not persist Source paths or membership, hover and focus state, open
menus, filter text, scroll positions, cursors, transient errors, read results,
pending tasks, or other ephemeral interaction state.

Autosave coalesces semantic changes, serializes off the GPUI thread, and
atomically replaces the local file. Invalid or unsupported schema versions are
reported without overwriting the offending document. The legacy
`seex-workbench 1` format is not migrated; this is an intentional pre-1.0
format break.

## Export and Import

Export writes only a `workbench.toml` document. It never embeds `config.toml`,
Source paths, native data, or secrets.

Import first autosaves the current workbench, then preflights every Source alias
and Project ID. An imported alias may map to an existing Source under a
different local alias or to a newly configured Source. The confirmation view
shows all alias rewrites and any Projects that must be added to an allowlist.

After confirmation, Seex validates all mappings, atomically updates
`config.toml` when needed, rewrites references to local aliases, and replaces
the current workbench. A failure before completion leaves both the previous
configuration and workbench active.

Unavailable Source, Project, and Run references remain in the workbench and
are shown as unavailable. They become active again after remapping or data
recovery and disappear only through explicit cleanup or unimport.

## Acceptance Scenarios

- Both documents round-trip valid TOML and reject invalid or unsupported
  schema versions without overwriting input.
- Scope selection, config layering, alias conflicts, and path bases follow the
  global/project rules without merging workbenches.
- Global storage and S3 defaults are inherited field by field, project values
  override them, and explicit SDK arguments take final precedence.
- SDK reporting leaves all config and workbench files byte-for-byte unchanged.
- Alias suggestions handle collisions, remain stable across path changes, and
  can be edited before import.
- Selecting part of a Source imports and loads Runs only for those Projects.
- Reloading an invalid external edit retains the last valid live Sources.
- UI-driven config changes preserve comments and detect concurrent edits.
- Existing native storage and S3 fields retain their values and semantics when
  the desktop updates Source configuration.
- Exported workbenches contain no Source paths, Project allowlists, secrets, or
  transient UI state.
- Import can map an external alias to a differently named local alias and
  updates config and state as one confirmed operation.
- Missing references survive save, restart, and later recovery.
- Archive preserves Project membership; Remove Project unimports it and clears
  its workbench references.

## Non-goals

Version 1 does not provide database-backed UI state, automatic cloud or
shared-file sync, team sharing, embedded Project data, multiple named
workbenches, workbench merging, or migration from `seex-workbench 1`.
