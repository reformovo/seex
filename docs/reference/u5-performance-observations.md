# U5 qualification and performance observations

U5 completed the single-crate convergence without changing the catalog or
Parquet compatibility boundary and without adding a runtime dependency. Cargo
metadata reports four `0.1.0-beta.1` packages: only `seex` is publishable;
`seex-python`, `seex-app`, and `seex-plot` are private. Maturin maps the source
version to Python `0.1.0b1`. No tag, push, publication, or CI/release workflow
change was made.

## Package and Acceptance

`scripts/package_smoke.py` packaged 43 files, checked `Cargo.toml` and
`Cargo.toml.orig` for identity, path dependencies, and forbidden private
dependencies/source, then extracted the crate into a fresh temporary directory.
`cargo test --locked --all-features` passed there with 144 unit tests, 20 client
integration tests, the facade boundary test, and five doc compile-fail tests.

The final local gates passed formatting, warning-free Clippy, workspace check
and tests, SDK doc tests, warning-denied workspace rustdoc, Ruff, Pyright, and
pytest. `cargo tree -p seex-plot --edges normal` contains only `seex-plot`.
Fresh isolated environments installed the `0.1.0b1` wheel and sdist and passed
`scripts/wheel_smoke.py`. The installed wheel also passed online LTTB loading
and explicit offline-extension loading through `scripts/rc_lttb_validation.py`.

MinIO/S3 round-trip and live partition-pruning Acceptance could not run because
`SEEX_MINIO_ENDPOINT`, `SEEX_MINIO_BUCKET`, `SEEX_MINIO_ACCESS_KEY_ID`, and
`SEEX_MINIO_SECRET_ACCESS_KEY` were all unset. The required `mc` executable was
available. This environment blocker does not replace those release checks in U7.

## Compact dual-View RSS preservation

- Rolling baseline: `20a51dc3342a2e54a039d0c587e1253a466d0d76`
- Candidate: `0fab2af41727928083de6faa24f10f022378a2f5`
- Fixture: `viewer-4x2x100k-dual-v3`
- Scale: 4 Runs, 2 Metrics, 100,000 points per series, 2 Views, three zoom
  round-trips, and four-way reads
- Protocol: one bounded A-B-B-A preservation comparison
- Artifact: `/private/tmp/seex-perf/u5-f23ac7d/viewer-rss-preservation-result.json`
- Artifact SHA-256:
  `000fd0be15748e5497752490c1366a45a728338b51eaccfea5af28a75e92ffae`

Baseline peak/final captures were 441,221,120 and 472,039,424 bytes. Candidate
captures were 418,611,200 and 408,272,896 bytes. The baseline pair varied above
the fixed 2% cross-process noise limit, so both protected metrics were
unreliable and the comparison classified Inconclusive. It was not rerun, the
classification does not block U5, and `docs/performance-baselines.json` remains
unchanged because only Pass may advance the rolling baseline.
