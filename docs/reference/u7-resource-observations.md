# U7 resource and performance observations

This U7 slice qualifies the release resource boundary at candidate revision
`dddaddd8df74da78e470384792b04bfe3bc0a483`. It changes no public Rust or
Python API, package metadata, catalog, DuckLake, or Parquet compatibility
boundary. No tag, push, publication, or Profile was performed.

This revision's local correctness result is not final release qualification.
A later clean environment exposed an LTTB availability-probe defect that a
developer-machine extension cache had masked. The resource comparison remains
valid because it did not exercise that storage path; final correctness status
and the superseding revision are recorded in
[`u7-release-qualification.md`](u7-release-qualification.md).

## Release Acceptance

The candidate ran on macOS 26.3 (25D125), arm64, with Rust/Cargo 1.97.1 and
Python 3.13.2 through `uv`. These checks passed:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo check`
- `cargo test`
- `cargo test -p seex-app --features test-support`
- online and explicit offline LTTB loading through the ignored Rust Acceptance
- `uv run --group linting ruff format --check python scripts tests`
- `uv run --group linting ruff check python scripts tests`
- `uv run pyright`
- `uv run pytest`
- `uv run python scripts/package_smoke.py`
- `cargo build -p seex-app --release`
- `uv run python scripts/viewer_smoke.py`
- release wheel and sdist builds, isolated Python 3.13 installs, and
  `scripts/wheel_smoke.py` against each installed package

The standalone crates.io package contained 43 files and independently passed
145 unit tests, 20 client integration tests, the facade test, the benchmark
record test, and five compile-fail doc tests. The wheel and sdist both
identified as Python `0.1.0b1`; the corresponding local artifact SHA-256
values were `ed033c64ddaa656c91df1ec943ddd19b2beb1dc748803364eb330938b1177fe0`
and `09ef45e794ff07707ab2d40baf1cd93be65eb6706c7a2b6756deb7cb566dcb0a`.

The first standalone package attempt exhausted the local temporary volume
while `ranlib` wrote bundled DuckDB (`errno=28`). After clearing only Cargo's
reproducible dev-profile output, the exact command passed. The first installed
package smoke invocations used HOME paths that had not been created and failed
before storage initialization; both packages passed after the isolated HOME
directories were created and the smoke was rerun with immediate failure
propagation. Neither incident was a product assertion failure.

## Stale and cancelled resource ownership

Ordinary small-fixture tests, included in the passing Rust suites above, prove
the resource contract without an RSS stress matrix:

- `inspector_stops_when_superseded_between_storage_queries` returns no
  snapshot after cancellation between native storage queries.
- `pending_requests_keep_the_latest_generation_per_kind` and
  `pending_curve_requests_coalesce_per_metric_panel` discard superseded queued
  work while retaining only the latest bounded request per panel identity.
- `superseded_and_inactive_view_results_are_ignored` observes two stale reads
  and exactly zero stale retained snapshots.
- `deactivating_a_view_cancels_every_panel_generation` clears Overview,
  Detail, and Inspector pending generations and the requested Detail viewport.

## Compact dual-View RSS comparison

- Rolling baseline: `12f6a555d0b5ad4a9891ffaf3de2efcb7e740f26`
- Candidate: `dddaddd8df74da78e470384792b04bfe3bc0a483`
- Fixture: `viewer-4x2x100k-dual-v3`
- Scale: 4 Runs, 2 Metrics, 100,000 points per series, 2 Views, three zoom
  round-trips, and four-way reads
- Protocol: one bounded schema-v3 A-B-B-A preservation comparison
- Artifact:
  `/private/tmp/seex-perf/u7-dddaddd/viewer-rss-preservation-result.json`
- Artifact SHA-256:
  `c8658bedc5e62c70cf6c570b86f1c2ede70b57b433c7d1c6491b9aab002ab206`
- Verdict: **Inconclusive** — selected metrics exceeded the fixed noise limit

Baseline peak/final captures were 526,893,056 and 414,924,800 bytes;
candidate captures were 515,063,808 and 426,377,216 bytes. The ordered pairs
changed by -2.25% and +2.76%, while the combined candidate median was 0.04%
lower. Cross-process relative MAD was 11.89% for the baseline and 9.42% for
the candidate, above the fixed 2% limit, so the result does not establish a
regression or advance `viewer.rss.peak` in `performance-baselines.json`.

An initial invocation failed before emitting an RSS phase because the U6
baseline helper read the real user configuration and did not settle before
its test deadline. That isolation defect is fixed by `e5106ba`; the comparison
then gave both exact-revision processes separate existing empty HOME
directories. The failed invocation produced no capture and is not a completed
workload. The A-B-B-A above is the only completed U7 performance workload.

No reporting, Reader, or Viewer chart CPU hot path changed after its rolling
revision, so no comparison was run for those boundaries. Acceptance passed
and the RSS observation was noisy rather than diagnostic of a regression;
therefore Instruments, Metal, allocation, display Profiles, and stress
workloads were unnecessary.
