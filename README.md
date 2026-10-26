# Seex

Seex (pronounced “six”, `/sɪks/`) is a local-first training metrics tracker
backed by Rust, PyO3, DuckDB, and DuckLake.

> [!IMPORTANT]
> Experimental. Seex is in the 0.x line. Pre-1.0 releases do not promise
> store, API, or machine-output compatibility; breaking changes between 0.x
> releases will not preserve compatibility, and compatibility and migration
> commitments begin with the future 1.x line.

Seex 0.1.0b6 beta surface:

- use the typed Run writing surface from Python or Rust
- log run-scoped numeric metric mappings with explicit or implicit steps
- keep chart reads in the native Rust Reader and Viewer
- store Runs in a local SQLite WAL catalog with DuckLake-managed local data
- inspect Step-aligned multi-Run curves, up to 16 Runs and 1,024 points per curve
- preserve every repeated Step observation in the append-only v5 store
- keep the Parquet schema as the long-term compatibility boundary

Install the Python beta with `pip install seex==0.1.0b6`, or add the Rust SDK
with `cargo add seex@0.1.0-beta.6`.

Known limits: this release opens only native v5 stores created with the bundled
SQLite catalog and local data path. Legacy DuckDB catalogs, S3 data paths, and
older store formats are rejected without migration. Charts are Step-only; the
Elapsed time control is visibly disabled. See the
[0.1.0b6 release notes](docs/release-notes/0.1.0b6.md) for the full
compatibility and performance limits.

Quickstart:

```python
import seex

with seex.init(project="training", dir="runs", id="baseline") as run:
    run.log({"train/loss": 0.25, "samples": 64}, step=0)
    run.log({"train/loss": 0.125})  # Implicitly advances to step 1.
```

Reduced curve queries use the Seex-native LTTB selector. Summaries and reporting
remain full-fidelity. Non-finite observations are filtered at the storage
boundary, and `source_count` reports finite observations in range. Reduced reads
require no external DuckDB extension.

The `seex` command only launches the native Viewer:

```console
seex app runs
```

Launch an independently installed desktop app with `seex app`, optionally
opening one project path with `seex app <project-path>`. The SDK wheel does not
bundle the `seex-app` binary. On Apple Silicon macOS, install the Viewer and
launch its app bundle with Homebrew:

```console
brew install reformovo/tap/seex-app
open "$(brew --prefix seex-app)/Seex.app"
```

GitHub prereleases provide the ad-hoc-signed macOS ARM64 files
`Seex-macos-aarch64.zip` and `Seex-macos-aarch64.zip.sha256`. After verifying
the checksum, extract the archive and open `Seex.app`. Direct downloads are not
Apple-notarized and may require a one-time Gatekeeper approval. The Homebrew
Formula builds the app bundle from source and also exposes `seex-app` on
`PATH`; the Python wheel and Viewer remain separate artifacts.

By default, Seex stores local state under `./.seex` and uses the
`"uncategorized"` project. Pass `dir` and `project` explicitly when needed:

```python
run = seex.init(project="training", dir="runs")
```

Use `resume=True` or `resume="allow"` to resume a matching active Run or create
it when missing. `resume="must"` requires an explicit `id` and an existing
resumable Run. The default, `resume=None`, creates a new Run.

Seex does not discover or migrate legacy `.pulseon` state. Existing legacy
directories remain untouched; initialize a new `.seex` store instead.

`Settings` configures `catalog_backend`, `catalog_path`, `data_path`, and
`metric_queue_capacity`. This release accepts only the local SQLite catalog and
a local `data_path`; remote `s3://` data and catalog paths are rejected when
the native store opens, and no object-storage settings are accepted.

Project-local storage settings can live in `./.seex/config.toml`. Relative
`data_path` and `catalog_path` values in this file are resolved from the project
root passed as `dir` to `seex.init(...)`:

```toml
catalog_backend = "sqlite"
data_path = "data"
```

Explicit `Settings` values override values from `config.toml`:

```python
settings = seex.Settings(
    catalog_backend="sqlite",
    data_path="data",
    metric_queue_capacity=65536,
)
run = seex.init(project="training", dir="runs", settings=settings)
```

`run.finish()` marks the Run finished; a non-zero exit code marks it failed.
A normal context exit finishes the Run, while an exceptional exit marks it
failed and preserves the original exception. Finalization drains accepted
metrics and releases native writer resources.

Architecture entry points:

- [Docs index](docs/README.md)
- [Native storage boundary](docs/native-storage-boundary.md)
- [Glossary](docs/glossary.md)
- [Roadmap](docs/ROADMAP.md)
- [ADRs](docs/adr/)

Runtime dependencies:

- DuckLake is installed and loaded by the native engine because it is required
  for native storage.
- Curve sampling uses the bundled Seex-native LTTB selector; no external DuckDB
  LTTB extension is required. Chart queries require an explicit `max_points`
  from 2 through 1024, and the bound is strict.
- Seex embeds DuckDB 1.5.4. DuckDB extension binaries are specific to a DuckDB
  version and platform; use the bundled native path for offline deployment.
