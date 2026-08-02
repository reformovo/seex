# U4 Performance Observations

## Qualification

- Candidate: `19f00aba232557dc71a155c3c137ff3bb3281df6` on macOS arm64.
- Correctness gates passed `cargo check`, complete `cargo test`, workspace
  Clippy with warnings denied, Ruff format/lint, Pyright, and 131 Python tests;
  two opt-in MinIO tests were skipped because MinIO was not configured.
- A wheel and sdist were each installed into a fresh Python 3.13 environment
  and passed `scripts/wheel_smoke.py`.
- Each comparison used one A-B-B-A order, ten internally calibrated samples of
  at least 25 ms per capture, atomic checkpoints, and the 120-second gate.
  The gate recorded the workspace as dirty only because an unrelated existing
  untracked draft was deliberately excluded from U4.

## Python reporting

- Explicit-single admission compared rolling revision
  `1886c7c72d034d86511a71eef517c544633ffc7e` with U4 using the
  `reporting-local-v3` fixture: 917,504 points per sample and ten samples per
  capture. It was a non-blocking Regression. Candidate medians were 474,594
  and 470,447 points/s versus baseline medians 1,514,855 and 1,480,109
  points/s; the combined regression was 68.45%, with both order pairs agreeing.
  Artifact SHA-256:
  `2f92f29f173d24487d3dfe6344618e46b88c51e1d86a5007dfd4d9f5e6876668`.
- Durability compared rolling revision
  `e34e351d85f1ce324b31fb890fd0cf79be7aba3b` with U4 using 1,000 reports
  per Run, three Runs per sample, and ten samples. It was Inconclusive because
  drain persistence and finalization both exceeded the 2% noise limit.
  Artifact SHA-256:
  `01584025522485ca27497c20d741b50b3ebbe76ecfa6c050739ec8e2851267f9`.
- Both observations are non-blocking. Neither reporting rolling baseline
  advanced.

## DuckDB Reader Step

- Reader Step compared rolling revision
  `9edd1cdb9c34f16b7064d782edfb0ce74ac2fad4` with U4 on the retained
  `reader-1x1x1m-v3` fixture, measuring narrow detail and full overview ranges.
- Preservation passed. Narrow combined change was +0.14%, with pair changes
  +0.19% and +0.08%. Full combined change was -40.07%, with both pairs near a
  40.1% improvement. Maximum capture relative MAD was 1.31%.
- Artifact SHA-256:
  `e311128c2f436d1c04aad56f6d59b6ae12eed3353cc38b2d75566a6c44f0af9e`.
  Because the verdict was Pass, `docs/performance-baselines.json` advanced only
  the DuckDB Reader Step rolling baseline to the U4 revision.
