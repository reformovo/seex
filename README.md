# Seex

Seex (pronounced “six”, `/sɪks/`) is a local-first training metrics tracker
backed by Rust, PyO3, DuckDB, and DuckLake.

> [!IMPORTANT]
> Experimental. Seex is in the 0.x line. Pre-1.0 releases do not promise
> store, API, or machine-output compatibility; breaking changes between 0.x
> releases will not preserve compatibility, and compatibility and migration
> commitments begin with the future 1.x line.

Seex 0.1.0b5 beta surface:

- use the typed Run writing surface from Python or Rust
- log run-scoped numeric metric mappings with explicit or implicit steps
- keep chart reads in the native Rust Reader and Viewer
- use DuckDB or SQLite catalogs with local or S3-compatible Parquet data
- inspect bounded multi-Run curves in the independently distributed macOS Viewer
- keep the Parquet schema as the long-term compatibility boundary

Install the Python beta with `pip install seex==0.1.0b5`, or add the Rust SDK
with `cargo add seex@0.1.0-beta.5`.

Known limit: with the default DuckDB catalog, an independent reader may not
attach or refresh while a writer is active. SQLite catalogs support one writer
process with multiple native Readers using WAL. See the
[0.1.0b5 release notes](docs/release-notes/0.1.0b5.md) for Viewer source
management changes and inherited compatibility and performance limits.

Quickstart:

```python
import seex

with seex.init(project="training", dir="runs", id="baseline") as run:
    run.log({"train/loss": 0.25, "samples": 64}, step=0)
    run.log({"train/loss": 0.125})  # Implicitly advances to step 1.
```

Reduced curve queries use the Seex-native LTTB selector. Full reads, scalar
summaries, and reporting remain full-fidelity. Reduced reads require no external
DuckDB extension; non-finite observations remain part of source counts and make
the series ineligible for bounded sampling.

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

`Settings` configures `data_path`, `catalog_backend`, `catalog_path`,
`metric_queue_capacity`, and S3 connection fields. `catalog_path` must be a
local filesystem path. `data_path` may be local or an S3-compatible URI such
as `s3://bucket/prefix`. Credential values are redacted from `repr(settings)`.

Project-local storage settings can live in `./.seex/config.toml`. Relative
`data_path` and `catalog_path` values in this file are resolved from the project
root passed as `dir` to `seex.init(...)`:

```toml
data_path = "s3://example-bucket/seex/demo"

[s3]
endpoint = "https://s3.example.com"
region = "us-east-1"
access_key_id = "<access-key-id>"
secret_access_key = "<secret-access-key>"
path_style = true
use_ssl = true
```

Do not commit real S3 credentials. Explicit `Settings` values override values
from `config.toml`:

```python
settings = seex.Settings(
    data_path="s3://example-bucket/seex/demo",
    s3_endpoint="https://s3.example.com",
    s3_access_key_id="<access-key-id>",
    s3_secret_access_key="<secret-access-key>",
    s3_path_style=True,
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

Runtime extensions:

- DuckLake is installed and loaded by the native engine because it is required
  for native storage.
- DuckDB LTTB is optional and is not bundled into Seex wheels. Native Readers
  use an available compatible `lttb` extension when applicable. Chart queries
  require an explicit `max_points` from 2 through 1024, and the bound is strict.
- Seex embeds DuckDB 1.5.4 and delegates signed community-extension
  compatibility to DuckDB and the community extension repository rather than
  duplicating their platform matrix in Seex's generated CI. DuckDB extension
  binaries are specific to a DuckDB version and platform; for offline
  deployment, set
  `SEEX_LTTB_EXTENSION_PATH=/path/to/lttb.duckdb_extension` to a compatible,
  signed binary rather than reusing one built for another DuckDB version.

See DuckDB's [LTTB extension page][lttb] and [extension installation
guide][duckdb-extension-install] for the upstream commands and compatibility
rules.

[lttb]: https://duckdb.org/community_extensions/extensions/lttb.html
[duckdb-extension-install]: https://duckdb.org/docs/current/extensions/installing_extensions.html
