---
name: land
description: >-
  Land an explicitly requested change in the Seex repository: prepare the
  current branch, run the repository's local release gates, open or update a
  pull request against main, wait for the required CI checks on the exact head
  commit, squash-merge it, and verify the merged main history. Invoke only when
  the user explicitly asks to land or merge the requested changes, not merely
  for review, preparation, passing checks, or skill installation.
disable-model-invocation: true
metadata:
  delta-action: land
---

# Land Seex changes

Use this workflow when the user has explicitly asked to land, merge, or ship the
changes in this thread. The request itself supplies landing intent; do not stop
to ask whether they want to merge. Continue through verification, merge, and
post-merge confirmation unless a genuine blocker appears.

The landing target is `reformovo/seex`'s `main` branch through a pull request.
Recent `main` history uses single-parent PR commits, so use a squash merge. The
repository has no PR template. At setup time the classic branch-protection
endpoint returned 404 and the ruleset list was empty; re-check both before
merging. The workflow and `AGENTS.md` still define the required quality gates. Do
not treat the absence of visible branch protection as permission to bypass checks
or push directly to `main`.

Tagging, publishing to crates.io or PyPI, GitHub releases, and Homebrew are
separate release operations. This skill stops after the change is merged and
verified on `main`. Perform those publication steps only on a separate explicit
request.

## 1. Identify the requested change

- Use the current branch unless the user names a different branch, commit, or
  existing PR.
- Remote `origin` points at `https://github.com/reformovo/seex.git`.
- If the current branch is `main`, create a landing branch from the current
  commits first (`git switch -c <name>`) and push that branch; never push `main`
  directly.
- If the request is ambiguous about which change to land, stop and ask. Do not
  guess by combining unrelated branches.
- Do not create a replacement branch merely to change the PR name.

## 2. Preflight the worktree and target

Run these checks from the repository root:

```console
git rev-parse --show-toplevel
git status --porcelain
git fetch origin main --tags
git rev-list --left-right --count origin/main...HEAD
git merge-base --is-ancestor origin/main HEAD
BRANCH=$(git branch --show-current)
if git show-ref --verify --quiet "refs/remotes/origin/$BRANCH"; then
  git merge-base --is-ancestor "origin/$BRANCH" HEAD
fi
```

The `merge-base` commands exit nonzero when the branch is behind or diverged.
That is a signal to apply the conditions below, not a generic command crash.

The branch must contain the latest `origin/main`, have at least one commit to
land, and be a fast-forward of its existing remote branch (if that branch
already exists). If `origin/main` is not an ancestor, go to section 8 and merge
it first; resolve only clear conflicts, then rerun the local gates. Stop and
report instead of merging when:

- The worktree has uncommitted changes. Do not discard, reset, clean, or commit
  them without a separate explicit request.
- After conflict handling, `origin/main` is still not an ancestor of `HEAD`.
- The remote branch exists but is not an ancestor of `HEAD`; a force-push would
  be required.
- There are no commits between `origin/main` and `HEAD`.
- The repository or branch is not the expected Seex checkout.

The repository rules forbid force-pushing, destructive resets, and deleting user
changes. Do not use `git reset --hard`, `git clean`, or `git push --force`.

## 3. Run local gates

Run these commands from the repository root and record the actual result of each
one. They mirror the PR quality jobs in `.github/workflows/ci.yml` (`rust` and
`python`) and the commands in `AGENTS.md`.

```console
cargo fmt --all -- --check
cargo check --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --release --workspace --all-features
uv sync --group testing-extra --group linting
uv run --group linting ruff format --check python scripts tests
uv run --group linting ruff check python scripts tests
uv run pyright
uv run --group testing-extra pytest
git diff --check
```

Sources: `.github/workflows/ci.yml` (`Rust checks` and `Python checks`),
`AGENTS.md` (Commands), and `docs/releasing.md` (local release gates). If the
change touches a measured hot path, follow the conditional performance section
below; never invent a benchmark result or substitute a diagnostic profile for an
ABBA comparison.

If any command fails, stop before pushing or merging. Report the exact command,
the failing output, and that the change has not landed. Do not weaken lint,
type-check, or test configuration to get past a failure.

Performance comparison is non-blocking and conditional, per `AGENTS.md`:

- Run one bounded A-B-B-A workload only when the change can affect a measured
  hot path and a valid manifest is available.
- Use `python scripts/performance_gate.py run --manifest <candidate.json>
  --output <result.json>`.
- Do not rerun to cherry-pick a result. Report the comparison outcome; it does
  not replace the required acceptance checks above.

## 4. Authenticate for push and PR operations

GitHub CLI is configured with two accounts. The active account may have only
read access, while recent PRs were authored and merged by `reformovo`.

1. Record the current account with `gh api user --jq .login`.
2. If it is not `reformovo`, switch for landing:
   `gh auth switch --hostname github.com --user reformovo`.
3. Verify the switch with `gh api user --jq .login`.
4. If the switch fails or the account cannot push/create/merge, stop and report
   the authentication blocker. Do not print or copy tokens.
5. Restore the original account in the cleanup section on every exit path.

The `origin` remote is HTTPS. Use the active `gh` account's token for the
landing push without persisting a credential-helper change:

```console
git -c credential.helper= \
  -c credential.helper='!gh auth git-credential' \
  push origin "$(git branch --show-current)"
```

Use `-u` instead of plain `push` when the branch has no upstream. If the push is
rejected, stop and report the credential problem; do not force-push or rewrite
history.

## 5. Push and create or reuse the PR

Push the branch now, using the credential-helper command from section 4. If the
branch has no upstream, use the `-u` form. Then check for an open PR for this
branch:

```console
gh pr list --head "$(git branch --show-current)" --base main --state open \
  --json number,url,headRefOid,baseRefName
gh pr list --head "$(git branch --show-current)" --base main --state merged \
  --limit 1 --json number,state,mergedAt,headRefOid,mergeCommit
```

If the most recent merged PR's `headRefOid` equals the current `HEAD`, the change
is already on `main`; report that result and stop without merging again. If a
merged PR has an older head, the branch contains newer work and can continue.

If no open PR exists, create one. If an open PR exists, reuse it and verify that
its base is `main` and its `headRefOid` matches `git rev-parse HEAD` after the
push. If the base is wrong or the head differs, stop and report; do not close or
replace the PR silently.

For the PR title, use a concise subject derived from the latest relevant commit.
For a release branch this is normally the latest `release: ...` subject; for a
feature branch use the latest `feat:`, `fix:`, `perf:`, or `refactor:` subject.
Do not use an interactive editor. Create the body from a non-interactive heredoc
with these sections:

- `## Summary`: the title-level change and the number of commits since
  `origin/main`.
- `## Verification`: the exact local commands that passed.
- `## Release notes`: link a changed file under `docs/release-notes/` when the
  range includes one; otherwise omit this section.

Create the PR with an explicit base, head, title, and body through a
non-interactive heredoc. Replace the placeholders; the release-notes section is
only for ranges that change a file under `docs/release-notes/`:

```console
gh pr create --repo reformovo/seex --base main \
  --head "$(git branch --show-current)" \
  --title "<derived title>" --body-file - <<'BODY'
## Summary

- <title-level change>
- <n> commits since `origin/main`

## Verification

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-features`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --release --workspace --all-features`
- `uv run --group linting ruff format --check python scripts tests`
- `uv run --group linting ruff check python scripts tests`
- `uv run pyright`
- `uv run --group testing-extra pytest`
- `git diff --check`

## Release notes

- <link to the changed release-notes file, when applicable>
BODY
```

`gh` must not prompt. Do not create a fork, open an editor, or use `-f` for a
large release branch. The release runbook requires implementation, metadata,
tests, and release notes to travel in one PR (`docs/releasing.md`, "Merge and tag
Seex").

## 6. Wait for the required CI checks

On the PR page, the PR quality jobs are `Rust checks`, `Python checks`, and the
aggregate `Required checks` job from `.github/workflows/ci.yml`. Matrix, build,
Viewer, release-guard, and publish jobs are skipped on pull requests; skipped
jobs are expected and do not indicate success of the required checks.

Record the PR head SHA and watch the checks:

```console
gh pr view <number> --repo reformovo/seex --json headRefOid,state,url
gh pr checks <number> --repo reformovo/seex --watch --fail-fast
gh pr checks <number> --repo reformovo/seex --json name,state,bucket
```

Use a bounded terminal timeout for the watch (for example 30 minutes). After the
watch completes, verify all of the following against the **current** head SHA:

- `Rust checks`, `Python checks`, and `Required checks` each have a successful
  result. Missing or skipped required checks are failures.
- No required check is pending, queued, in progress, cancelled, or failed.
- The PR's `headRefOid` still equals `git rev-parse HEAD`.
- The `Required checks` job is successful, not merely present.

If a check is pending, wait; do not merge early. If a check fails, is cancelled,
is missing, or the head SHA changed, stop and report the PR URL and failing
check. Do not use `--admin`, bypass rules, or merge on an earlier commit.

## 7. Merge and verify

After the required checks pass on the exact head commit, re-check the current
destination settings before merging. A 404 from the classic protection endpoint
means no classic protection; an empty ruleset list means no repository rulesets.
If rules or protection now exist, inspect them and satisfy required reviews,
checks, or merge-queue behavior before proceeding. If they require a manual
review or another condition this workflow cannot complete, stop and report.

```console
gh repo view reformovo/seex --json defaultBranchRef,squashMergeAllowed
gh api repos/reformovo/seex/rulesets
gh api repos/reformovo/seex/branches/main/protection
```

If squash merge is no longer allowed, stop and ask before choosing another
strategy. Otherwise merge with squash and bind the merge to the verified head:

```console
HEAD_SHA=$(git rev-parse HEAD)
gh pr merge <number> --repo reformovo/seex --squash \
  --match-head-commit "$HEAD_SHA"
```

Use `--squash` because recent `main` history shows single-parent PR commits. Do
not use `--merge`, `--rebase`, `--admin`, `--auto`, or `-d/--delete-branch`
unless the user separately requests a different strategy or branch cleanup. If
the repository now requires a merge queue, `gh pr merge` may only enqueue the PR;
poll `gh pr view <number> --json state` with a bounded timeout and do not report
success until the state is `MERGED`.

If the merge reports a conflict, permission failure, or stale head, do not retry
blindly. Resolve only clear conflicts as described below, rerun the local gates,
push the resolution, and wait for fresh CI. Otherwise stop and report.

After the merge, fetch and verify the destination:

```console
git fetch origin main
gh pr view <number> --repo reformovo/seex --json state,mergeCommit,url,baseRefName
git cat-file -e <merge-commit-sha>^{commit}
git merge-base --is-ancestor <merge-commit-sha> origin/main
git log -1 --oneline origin/main
```

Success requires the PR state `MERGED`, base `main`, a merge commit reachable
from `origin/main`, and the latest `origin/main` subject containing the PR
number. The local branch may remain checked out; do not switch branches or delete
the remote branch unless the user explicitly asks. A squash merge does not make
the branch's original commits ancestors of `main`; do not reuse this branch for
further work without creating a fresh branch or explicitly resyncing it.

## 8. Conflict handling

The user preference is to resolve conflicts automatically only when the intended
result is clear, and otherwise pause.

If `main` advanced or the PR reports conflicts:

1. Fetch `origin/main` and merge it into the current branch with `git merge
   origin/main`. Do not rebase or force-push.
2. Inspect each conflict. If the correct result is unambiguous from the two
   sides, preserve both intended changes, stage the file, and continue the merge
   with a non-interactive commit message or `git merge --continue`.
3. If the correct result is ambiguous, stop immediately and report the conflict
   paths and the two competing intents. Leave the conflict state for the user;
   do not choose a side, reset, or abort unless they ask.
4. After a clear resolution, rerun every local gate in section 3, push the merge
   commit (fast-forward only), wait for fresh CI on the new head, and return to
   section 6/7.
5. If validation fails or the new head's checks fail, stop and report. Do not
   merge an unresolved or unverified conflict.

## 9. Cleanup and reporting

Always restore the original GitHub CLI account on exit, including failure paths:

```console
gh auth switch --hostname github.com --user "<original-account>"
```

Then report:

- Whether the change landed, and the PR URL.
- The merged commit SHA and confirmation that it is on `origin/main`.
- The local gate commands that passed.
- The required CI checks that passed, by name.
- Any conflict resolution performed.
- Any blocking failure that prevented landing.

Never claim success from a prepared commit, a pushed branch, a created PR, or
pending checks. Landing succeeds only after the merge is visible on
`origin/main`.

Do not tag the merge, publish packages, create a GitHub release, update the
Homebrew formula, or delete branches as part of this skill unless the user makes
a separate explicit request.
