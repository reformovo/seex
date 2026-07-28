## Stack

- Python package backed by Rust/PyO3, built with `maturin`.
- Virtual Cargo workspace: edition 2024 crates under `crates/`; the PyO3
  `cdylib` source is under `crates/pulseon-python/src/`.
- Python package source: `python/pulseon/`; tests under `tests/`.
- Architecture references: `docs/native-storage-boundary.md` and
  `docs/crate-boundaries.md`; roadmap: `docs/ROADMAP.md`.

## Must Always

- Declare the files you will edit before changing code. Keep normal tasks to <=5 files and <=200 changed lines unless the user expands scope.
- Use the root `rust-toolchain.toml` as the single Rust toolchain source for local development and CI; update it deliberately when upgrading Rust.
- Preserve the native storage boundaries described in `docs/native-storage-boundary.md`.
- Keep Python code typed; update `python/pulseon/_pulseon.pyi` when exposing new Python API from Rust.
- Type-check all Python code under `python/`, `scripts/`, and `tests/`; keep suppressions narrow and inline for deliberate type-contract violations.
- Add or update tests for new behavior. Rust logic should have Rust tests where possible; Python-facing behavior should have `pytest` coverage.
- Run the relevant verification commands after edits and report any command you could not run.
- Update this file in the same change when project conventions or required commands change.

## Must Never

- Do not edit generated build artifacts in `dist/`, `target/`, `pulseon.egg-info/`, `__pycache__/`, or `.pytest_cache/`.
- Do not commit secrets, `.env` files, local database files, or local object-storage data.
- Do not bypass failing checks by weakening lint/type/test configuration without explicit approval.
- Do not introduce broad refactors while implementing a narrow roadmap phase.
- Do not push, force-push, reset, or delete user changes unless explicitly asked.

## Boundaries

- Ask first before adding runtime dependencies, changing the public Python API shape, altering package metadata, or changing CI/release behavior.
- Prefer small roadmap-aligned phases from `docs/ROADMAP.md`; one feature/fix should be independently reviewable.
- Treat DuckLake as a required native dependency, but keep the Parquet schema as the product compatibility boundary.

## Commands

- Rust type-check: `cargo check`
- Rust tests: `cargo test`
- Python format: `uv run --group linting ruff format --check python scripts tests`
- Python lint: `uv run --group linting ruff check python scripts tests`
- Python type-check: `uv run pyright`
- Python tests: `uv run pytest`
- Develop install: `uv run maturin develop --uv`
- Wheel build: `uv run maturin build --out dist`

## Definition of Done

- Edited files stayed within the declared scope or scope expansion was explained.
- Relevant type-check, lint, test, and build commands pass, or failures are documented with exact blockers.
- New code follows the existing module layout and does not add unrelated abstractions.
- The diff is small enough to explain line by line.

## Skills

- Python code writing, reviewing, testing, or refactoring -> use `.agents/skills/python-best-practices/`.
- Rust code writing, reviewing, testing, or refactoring -> use `.agents/skills/rust-best-practices/`.
- GPUI application code, actions/keybindings, async tasks, contexts, elements,
  focus, layout, element IDs, or GPUI tests -> use `.agents/skills/gpui/`.
- Domain terminology, ubiquitous language, glossary, or ADR maintenance -> use `.agents/skills/domain-modeling/`.
- Vibe coding workflow, AI coding constraints, scope lock, verification gates, or contract maintenance -> use `.agents/skills/reform/`.
- Architecture grilling, ADRs, glossary, or plan hardening -> use `.agents/skills/grill-with-docs/`.
