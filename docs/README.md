# Seex Docs

This directory keeps the current product docs, accepted decisions, release
notes, and historical architecture snapshots.

## Current Docs

- `docs/ROADMAP.md` for the current release plan and later backlog.
- `docs/comparison-semantics.md` for the renderer-agnostic comparison contract.
- `docs/native-storage-boundary.md` for the current native storage boundary.
- `docs/crate-boundaries.md` for workspace crate responsibilities and dependency
  direction.
- `docs/catalog-application-tables.md` for project, run, and aggregate catalog
  table schemas.
- `docs/glossary.md` for product terms.
- `docs/parquet-schema-contract.md` for the Parquet compatibility contract.
- `docs/releasing.md` for the crates.io, PyPI, GitHub, macOS Viewer, and
  Homebrew release runbook.
- `docs/adr/` for accepted decisions.
- `docs/release-notes/` for shipped release notes and completed roadmap
  history.

## Historical Context

PulseOn was the project name through version 0.1.1. Seex restarts the package
and storage identity at 0.1.0b0 without a compatibility or migration layer; see
[ADR 0014](adr/0014-rename-pulseon-to-seex.md).

- `docs/reference/native-architecture.md` records the completed first native
  architecture.
- `docs/reference/gpui-single-panel-plan.md` records the original GPUI
  single-panel implementation plan.
- `docs/reference/benchmark-report.md` preserves the last pre-rename benchmark record.
- `docs/reference/ducklake-archive.md` preserves DuckLake validation notes that
  support the storage boundary.
