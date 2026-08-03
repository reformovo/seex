# U7 release qualification

This record fixes the qualification protocol for Seex 0.1.0 beta.1. The
qualified revision is the commit containing this document and the matching
U7 roadmap update. No source, package, or workflow change may follow the
qualification run; a required fix starts qualification again from its new
HEAD.

## Release identity and authority boundary

- Cargo packages identify as `0.1.0-beta.1`, Python distributions identify as
  `0.1.0b1`, and the only permitted release tag is `v0.1.0-beta.1`.
- `seex` is the only crates.io package. The Viewer is an unsigned macOS ARM64
  artifact and never enters the Python artifact or PyPI channel.
- The release graph is `release-guard -> crates.io -> PyPI -> GitHub
  prerelease`; Python and Viewer artifacts receive separate attestations.
- The crates.io API returned no package named `seex` during U7 preparation.
  If publication later reports a conflict, stop instead of selecting another
  name.
- Qualification does not authorize a push, tag, publication, or release.

## Acceptance matrix

All commands below must pass on the unchanged qualification revision. Command
output from that run is the execution evidence; checked roadmap items record
the accepted result without embedding machine-specific build output here.

| Boundary | Qualification command |
| --- | --- |
| Rust format and build | `cargo fmt --all --check`; `cargo check --workspace --all-targets --all-features` |
| Rust lint, tests, docs | `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --all-features`; `RUSTDOCFLAGS="-D warnings" cargo doc -p seex --no-deps` |
| LTTB policy | `cargo test -p seex lttb_online_and_explicit_offline_paths -- --ignored --nocapture` |
| Python quality | `uv run --group linting ruff format --check python scripts tests`; `uv run --group linting ruff check python scripts tests`; `uv run pyright`; `uv run pytest` |
| Public crate | `uv run python scripts/package_smoke.py` |
| Viewer | `cargo build -p seex-app --release`; `uv run python scripts/viewer_smoke.py` |
| Python artifacts | Build release wheel and sdist, install each into a clean Python 3.13 environment, then run `scripts/wheel_smoke.py` |
| S3/catalog | Against a unique temporary bucket in Apple `container` service `swanlab-minio`, run `tests/test_minio_acceptance.py`, `tests/test_catalog_backend_parity.py`, and `scripts/accept_minio_partition_pruning.py` |
| Installed-wheel Reader | Run `scripts/rc_lttb_validation.py` from the wheel environment against the same temporary MinIO configuration |
| Release identity | Stage isolated Python and Viewer artifact directories, run `scripts/release_guard.py --tag v0.1.0-beta.1`, and verify the Viewer SHA-256 checksum |

The MinIO run obtains credentials without printing them, creates a uniquely
named bucket, and removes that bucket and the temporary `mc` alias afterward.
It does not enumerate, read, or alter SwanLab buckets.

## Resource and profile decision

The required compact 4 Run x 2 Metric x 100,000 point dual-View workload and
ordinary stale/cancellation fixtures are recorded in
[`u7-resource-observations.md`](u7-resource-observations.md). The single
completed RSS A-B-B-A was Inconclusive under the fixed noise policy and is
non-blocking. No reporting, Reader, or Viewer CPU hot path changed after its
rolling revision. No failure required an Instruments, Metal, allocation,
display, or stress Profile, so no additional performance workload is allowed
for this candidate without new evidence.

## Tag gate

After every matrix entry passes, verify the worktree is clean and record the
exact `git rev-parse HEAD`. Stop there. Creating and pushing
`v0.1.0-beta.1`, or publishing any artifact, requires explicit user
authorization. The tag must point to that exact qualified revision.
