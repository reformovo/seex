# Release Runbook

This runbook publishes one immutable Seex revision to crates.io, PyPI, GitHub Releases, and `reformovo/tap`. Run it in order. Never move or reuse a published tag or package version.

## Release identity and prerequisites

One release has three equivalent identities:

| Channel | Example |
| --- | --- |
| Cargo | `0.1.0-beta.3` |
| Python | `0.1.0b3` |
| Git tag | `v0.1.0-beta.3` |

```console
export CARGO_VERSION=0.1.0-beta.3
export PYTHON_VERSION=0.1.0b3
export TAG=v0.1.0-beta.3
```

The operator needs an authenticated `gh` CLI, access to both repositories, and permission to approve the protected `release` environment. Secrets and PyPI trusted publishing must already be configured.

## 1. Confirm the identity is unused

```console
curl -A 'seex-release-check/1.0' \
  "https://crates.io/api/v1/crates/seex/${CARGO_VERSION}"
curl "https://pypi.org/pypi/seex/${PYTHON_VERSION}/json"
gh api "repos/reformovo/seex/git/ref/tags/${TAG}"
gh api "repos/reformovo/seex/releases/tags/${TAG}"
```

Each request must report that the identity does not exist. Start from a clean branch based on the latest `main`.

## 2. Synchronize release metadata

Update the Cargo version in `crates/seex/Cargo.toml`, `crates/seex-python/Cargo.toml`, `crates/seex-plot/Cargo.toml`, and `crates/seex-app/Cargo.toml`. Also update `Cargo.lock`, `README.md`, `tests/test_release_guard.py`, and add `docs/release-notes/<python-version>.md`.

The notes must name all three identities and user-visible changes. Search for stale current-version references without altering historical release notes:

```console
rg -n '0\.1\.0(-beta\.[0-9]+|b[0-9]+)|v0\.1\.0-beta\.[0-9]+' \
  Cargo.lock README.md crates tests
```

## 3. Run local release gates

```console
cargo fmt --all --check
cargo check --workspace --all-features
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
uv run --group linting ruff format --check python scripts tests
uv run --group linting ruff check python scripts tests
uv run pyright
uv run pytest
git diff --check
```

Run a performance comparison only for a measured hot path, following `docs/performance-validation.md`.

Build from the application crate directory. In `cargo-bundle 0.11.0`, icon paths are relative to the current directory; the workspace root silently omits the icon.

```console
(cd crates/seex-app && cargo bundle --release --format osx)
codesign --force --deep --sign - target/release/bundle/osx/Seex.app
codesign --verify --deep --strict --verbose=2 \
  target/release/bundle/osx/Seex.app
test "$(plutil -extract CFBundleIconFile raw \
  target/release/bundle/osx/Seex.app/Contents/Info.plist)" = Seex.icns
test -s target/release/bundle/osx/Seex.app/Contents/Resources/Seex.icns
```

For lifecycle changes, verify that last-window close retains the process, Dock activation restores a window in that process, and `Command-Q` exits.

## 4. Merge and tag Seex

Put implementation, metadata, tests, and notes in one PR. After required checks and merge, tag the exact merged `main`, not the feature-branch commit:

```console
git fetch origin main --tags
git tag -a "$TAG" origin/main -m "Release ${TAG}"
git push origin "refs/tags/${TAG}"
```

The tag workflow builds wheels, sdist, and Viewer, then follows:

```text
release guard -> crates.io -> PyPI -> GitHub prerelease
```

```console
gh run list --repo reformovo/seex --branch "$TAG" --limit 5
gh run watch <run-id> --repo reformovo/seex --exit-status
```

Both publish jobs use the protected `release` environment, so GitHub may request two approvals. For every pending deployment:

```console
gh api "repos/reformovo/seex/actions/runs/<run-id>/pending_deployments"
gh api --method POST \
  "repos/reformovo/seex/actions/runs/<run-id>/pending_deployments" \
  -F 'environment_ids[]=<environment-id>' -f state=approved \
  -f comment='Release checks passed; approve publication.'
```

Wait for the complete workflow to succeed.

## 5. Verify public artifacts

```console
curl -fsS -A 'seex-release-check/1.0' \
  "https://crates.io/api/v1/crates/seex/${CARGO_VERSION}"
curl -fsS "https://pypi.org/pypi/seex/${PYTHON_VERSION}/json"
gh release view "$TAG" --repo reformovo/seex --json \
  url,isPrerelease,isDraft,tagName,assets
```

The prerelease must contain `Seex-macos-aarch64.zip` and its `.sha256`. PyPI must contain only Python wheels and sdist; Viewer files must never enter its artifact glob.

## 6. Build and publish the Homebrew bottle

Compute the immutable tag archive checksum:

```console
export SOURCE_SHA=$(curl -fsSL \
  "https://github.com/reformovo/seex/archive/refs/tags/${TAG}.tar.gz" | \
  shasum -a 256 | awk '{print $1}')
echo "$SOURCE_SHA"
```

In `reformovo/homebrew-tap`, branch from `main` and update only `Formula/seex-app.rb`: replace tag/SHA and remove the old `bottle do` block. Keep the committed `resources/Seex.icns`; do not add `sips`, `inreplace`, `app-icon@2x`, or compatibility rewrites.

Run `brew style Formula/seex-app.rb`, push, and open a Formula PR. An installed tap checkout can validate it before CI:

```console
export TAP_BRANCH=<formula-pr-branch>
export TAP_CHECKOUT="$(brew --repository reformovo/tap)"
git -C "$TAP_CHECKOUT" fetch origin "$TAP_BRANCH"
git -C "$TAP_CHECKOUT" switch --detach "origin/${TAP_BRANCH}"
HOMEBREW_NO_AUTO_UPDATE=1 brew audit --strict --online reformovo/tap/seex-app
HOMEBREW_NO_AUTO_UPDATE=1 brew reinstall --build-from-source \
  reformovo/tap/seex-app
HOMEBREW_NO_AUTO_UPDATE=1 brew test reformovo/tap/seex-app
git -C "$TAP_CHECKOUT" switch main
```

Wait for PR `test-bot`; it builds, tests, audits, and uploads a bottle artifact. Do not manually merge the PR. Dispatch `publish.yml` with its exact head SHA:

```console
gh pr view <pr-number> --repo reformovo/homebrew-tap \
  --json headRefOid,statusCheckRollup
gh workflow run publish.yml --repo reformovo/homebrew-tap --ref main \
  -f pull_request=<pr-number> -f head_sha=<exact-pr-head-sha>
```

`brew pr-pull` uploads to GHCR, writes the bottle stanza to `main`, and closes the PR. Confirm the stanza uses `https://ghcr.io/v2/reformovo/tap`.

## 7. Verify the bottle installation

```console
brew update
brew reinstall reformovo/tap/seex-app
brew test reformovo/tap/seex-app
brew info --json=v2 reformovo/tap/seex-app
APP="$(brew --prefix seex-app)/Seex.app"
codesign --verify --deep --strict --verbose=2 "$APP"
test "$(plutil -extract CFBundleShortVersionString raw \
  "$APP/Contents/Info.plist")" = "$CARGO_VERSION"
test "$(plutil -extract CFBundleIconFile raw \
  "$APP/Contents/Info.plist")" = Seex.icns
test -s "$APP/Contents/Resources/Seex.icns"
! xattr -p com.apple.quarantine "$APP"
open "$APP"
```

The record must report `built_as_bottle: true`, `poured_from_bottle: true`, and no runtime dependencies. `Seex.app` is under the Homebrew prefix, not `/Applications`; the bottle does not install Rust or LLVM.
Source validation may install build tools locally; remove only packages newly
installed by that validation and unused by other formulae.

## Failure rules

- Never move a tag or overwrite a crates.io/PyPI version; use a new beta.
- If no source change is needed, fix infrastructure and rerun the immutable workflow.
- If source changes are required, create a new version and tag.
- Never hand-edit a bottle checksum; regenerate it through `test-bot` and
  `brew pr-pull`.
