# Static Web Viewer and Direct DuckLake Sources

> Status: Draft. This note is exploratory and does not change the accepted
> desktop-first decision, native storage boundary, Roadmap order, or product
> compatibility contracts.

## Summary

Seex may publish `seex.dev` as a static, client-executed analysis application
that hosts versioned application assets but no account, catalog, query,
compute, credential, proxy, or user-data storage service.

The browser should first attempt to open an existing Seex DuckLake Source
directly. A new exported Source format is not a prerequisite. DuckDB-Wasm can
run in a Web Worker, load a matching DuckLake WebAssembly extension, attach a
local or remote catalog read-only, and expose `dl.metric_points` together with
the Seex-owned catalog application tables.

## Roadmap Position

The fixed U4 through U7 order remains. The Roadmap places data export and Web
UI in independent phases after beta qualification. Unless a new ADR supersedes
desktop-first, this work starts after U7 with a bounded compatibility spike.

- Reader owns axes, half-open ranges, strict point bounds, real samples,
  completeness, and evidence.
- Native and Web query implementations must agree on last-write-wins,
  neighbors, diagnostics, and stale cancellation.
- U5 keeps `seex-plot` free of SDK, storage, PyO3, GPUI, and browser dependencies.
- U6 workbench references use Source aliases rather than paths.

## Working Terminology

The canonical **Data source** is currently local-native. This draft proposes a
broader term but does not update `CONTEXT.md` before an ADR is accepted.

- **Web Source**: one read-only Seex DuckLake made available to the Web Viewer.
- **Source transport**: how a browser reaches a Web Source, either through a
  user-authorized local directory or remote HTTP/S3-compatible objects.
- **Frozen Source**: an immutable Web Source backed by a read-only catalog and
  immutable Parquet objects, applying the official Frozen DuckLake pattern.

A Source alias remains the stable workbench identity. A local directory handle
or remote URL is a locator for that alias, not part of workbench state.

## Product Boundaries

### Goals

- Analyze local and user-hosted cloud Sources without sending data to Seex.
- Reuse the existing DuckLake catalog, logical tables, and query semantics.
- Preserve the current catalog and Parquet schemas.

### Non-goals

- A Seex-hosted query, proxy, credential, database, or object-storage service.
- Making GPUI, DuckDB-Wasm, or DuckLake part of the public `seex` Rust facade.
- Defining a portable export contract before direct Source access is tested.

## Proposed Architecture

```text
seex.dev static assets
  GPUI Web + DuckDB-Wasm worker + matching DuckLake extension
              |
              v
browser Source registry
  Source alias -> local directory handle or remote locator
              |
              v
DuckDB-Wasm Web Worker
  LOAD extension -> ATTACH READ_ONLY -> Reader semantics
              |
              v
Arrow batches / compact typed results
              |
              v
shared workbench snapshots -> seex-plot -> GPUI Web
```

## DuckLake-Wasm Feasibility

DuckDB-Wasm supports dynamic WebAssembly extensions. `INSTALL` is a browser
no-op; `LOAD` fetches, verifies, and links an Emscripten-built module. Copying a
signed extension to another static origin preserves its signature.

DuckLake is absent from the documented default extension set, but its official
CI builds `wasm_mvp`, `wasm_eh`, and `wasm_threads` artifacts. Seex should:

1. pin one DuckDB-Wasm engine revision and platform bundle;
2. pin the DuckLake extension built against the exact same DuckDB revision;
3. serve both from `seex.dev` instead of following `latest`; and
4. load explicitly, read-only, with automatic migration disabled.

Build availability is not runtime qualification. The spike must still prove
catalog attachment, data access, inline rows, remote paths, and Reader parity.

## Source Forms

### Local Source

The user selects a Source root through a directory picker or fallback. The app
registers its catalog and data under stable virtual paths, preserving layout:

```text
<root>/.seex/config.toml
<root>/.seex/catalog.ducklake
<root>/.seex/data/**
```

Attachment uses the registered catalog and virtual data path and should expose
inline and flushed rows through the same `dl.metric_points` relation.

The Web Viewer does not participate in native OS advisory locking. Initial
support should require a quiescent or consistently snapshotted catalog until
concurrent native-writer/browser-reader behavior is proven.

### Cloud Source

A cloud Source has an immutable or versioned catalog URL plus its recorded or
overridden data location. The browser accesses the object store directly.

The preferred model is a Frozen DuckLake catalog referencing immutable HTTP/S3
Parquet. New snapshots use new catalog objects rather than overwrite one being
queried. This requires neither catalog server nor Seex service.

Remote servers must support browser CORS and byte-range reads. Credentials are
provided by the user, remain browser-local, and are never sent to Seex.

## Catalog Backends

The first target is DuckDB catalog: `catalog.ducklake` contains DuckLake
metadata and all Seex-owned catalog application tables. SQLite is separate
acceptance because its complete browser path still needs validation.

## Compatibility and Safety

- Open user catalogs read-only and never run automatic DuckLake migrations.
- Pin DuckDB-Wasm and DuckLake extension ABI revisions together.
- Test catalog storage-format compatibility with every supported native writer.
- Reject an unsupported catalog or DuckLake schema without modifying it.
- Treat credentials, presigned URLs, and local directory handles as Source
  registry state, never workbench state.
- Serve COOP/COEP headers required by GPUI/DuckDB workers and SharedArrayBuffer.
- Load executable WASM only from pinned origins and verify no-upload behavior.

## Compatibility Spike

The first Web candidate must:

1. load a pinned, signed DuckLake extension in DuckDB-Wasm;
2. attach a real Seex DuckDB catalog read-only without migration;
3. discover Projects, Runs, and metric aggregates through Seex-owned tables;
4. query `dl.metric_points`, including persisted inline data;
5. query a terminal Run's partitioned local Parquet data;
6. attach an immutable remote catalog and read HTTP/S3 Parquet with CORS;
7. match native Reader results for last-write-wins, axes, ranges, neighbors,
   strict bounds, completeness, and diagnostics;
8. cancel stale work without publishing a stale workbench snapshot; and
9. retain no user data or credentials in Seex infrastructure.

Portable export is justified only if direct access fails a required boundary.

## Proposed Post-Beta Phases

- **W0 — DuckLake-Wasm compatibility:** execute the spike above with fixed
  local and Frozen Source fixtures.
- **W1 — Browser Reader:** implement the read-only Source adapter, parity tests,
  worker cancellation, and bounded query path.
- **W2 — GPUI Web Viewer:** add a private Web composition root, browser Source
  registry, workbench persistence, and shared renderer integration.
- **W3 — seex.dev qualification:** verify browsers, permissions, CORS/S3,
  no-upload behavior, performance, accessibility, and static release artifacts.

Portable export remains optional for interchange or backend-neutral archival.

## Open Questions

- Must Web v1 read a Source while a native writer is active?
- Which browsers, directory fallbacks, and cloud authentication modes are supported?
- May the browser cache catalogs or Parquet in OPFS, and under what control?
- Must SQLite catalog parity block the first Web release?
- How are native DuckDB/DuckLake versions mapped to Web reader bundles?
- Should Frozen Source become canonical terminology if this design is accepted?

## References

- [ROADMAP](../ROADMAP.md)
- [Desktop-first Viewer ADR](../adr/0011-desktop-first-curve-viewer.md)
- [Native storage boundary](../native-storage-boundary.md)
- [Crate boundaries](../crate-boundaries.md)
- [Viewer configuration and workbench state](viewer-configuration-and-workbench-state.md)
- [DuckDB-Wasm extensions](https://duckdb.org/docs/stable/clients/wasm/extensions)
- [Extensions for DuckDB-Wasm](https://duckdb.org/2023/12/18/duckdb-extensions-in-wasm.html)
- [DuckLake connection parameters](https://ducklake.select/docs/stable/duckdb/usage/connecting)
- [Read-only remote DuckLake](https://ducklake.select/docs/stable/duckdb/guides/using_a_remote_data_path)
- [Frozen DuckLakes](https://ducklake.select/2025/10/24/frozen-ducklake)
- [DuckLake WASM build evidence](https://github.com/duckdb/ducklake/actions/runs/30670594366)
