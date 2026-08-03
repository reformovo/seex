# U6 qualification and performance observations

U6 completed Source management, fixed Viewer scope, semantic workbench
snapshots, background autosave, unavailable-reference retention, and
transactional workbench export/import. It changed no public Rust or Python SDK
API, runtime dependency, catalog, DuckLake, or Parquet compatibility boundary.
No tag, push, publication, package metadata, or CI/release behavior changed.

## Acceptance

The final candidate was `12f6a555d0b5ad4a9891ffaf3de2efcb7e740f26` on
macOS 26.3 (25D125), arm64, with Rust/Cargo 1.97.1 and Python 3.13.2 through
`uv`. The performance runner used system Python 3.14.6. These commands passed:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo check`
- `cargo test`
- `cargo test -p seex-app --features test-support`
- `cargo build -p seex-app --release`
- `uv run python scripts/viewer_smoke.py`
- `uv run --group linting ruff format --check python scripts tests`
- `uv run --group linting ruff check python scripts tests`
- `uv run pyright`
- `uv run pytest`

The release smoke kept the Viewer alive through bounded observations in an
isolated global scope, an isolated Project scope, and an isolated scope with an
unsupported local workbench. GPUI Acceptance covered Source confirmation,
Manage Projects, reload fallback, archive/unimport, unavailable recovery,
autosave, export, alias rewrite, allowlist addition, missing Run retention,
transactional import, invalid external documents, and destination I/O failure.

One accidentally overlapping pytest confirmation run reported one transient
local `StorageError`. After the other process exited, the affected test passed
alone and the authoritative standalone full suite passed with 140 tests and 2
environment-dependent skips. No required verification was unavailable. Cargo
continued to report the existing future-incompatibility notices for `block`
0.1.6 and `proc-macro-error2` 2.0.1; warning-denied workspace Clippy passed.

## Compact dual-View RSS preservation

- Rolling baseline: `20a51dc3342a2e54a039d0c587e1253a466d0d76`
- Candidate: `12f6a555d0b5ad4a9891ffaf3de2efcb7e740f26`
- Fixture: `viewer-4x2x100k-dual-v3`
- Scale: 4 Runs, 2 Metrics, 100,000 points per series, 2 Views, three zoom
  round-trips, and four-way reads
- Protocol: one bounded schema-v3 A-B-B-A preservation comparison
- Artifact:
  `/private/tmp/seex-perf/u6-12f6a55/viewer-rss-preservation-result.json`
- Artifact SHA-256:
  `dc9fe6457955ec8bb55bbc0bd352c76279e69cb27f9d0942ac97ee78c309c3e4`
- Verdict: **Pass** — selected performance was preserved

Baseline peak/final captures were 483,639,296 and 469,434,368 bytes. Candidate
captures were 401,162,240 and 404,815,872 bytes. The ordered pairs improved by
17.05% and 13.77%, with a combined improvement of 15.43%. Baseline and
candidate cross-process relative MAD stayed below the fixed 2% noise limit.
The Pass advances `viewer.rss.peak` in `performance-baselines.json` to the U6
candidate.

An initial invocation failed before starting any workload because a historical
U5 baseline binary path no longer existed. It produced zero captures and was
discarded. The baseline was then rebuilt from the exact rolling revision; the
single completed A-B-B-A above is the only U6 performance workload.

The pure `seex-plot` chart CPU path was not modified, so no Viewer CPU
comparison was run. Profiles and stress workloads were unnecessary because
Acceptance and the affected RSS comparison passed.
