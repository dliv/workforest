# git-forest Bug: detect branch drift from forest metadata

## Summary

A forest worktree can be manually switched away from the branch recorded in
`.forest-meta.toml`. When that happens, `git forest ls`, `git forest status`,
and `git forest rm --dry-run --json` do not clearly report that the actual
checked-out branch differs from the expected forest branch.

This can leave a shared branch such as `main` checked out inside an old forest.
Git then blocks the primary checkout from using that branch:

```text
fatal: 'main' is already used by worktree at '/Users/dliv/oaw/.forests/sdt2y-release-tags/opencop-java'
```

The forest is still recoverable with `git forest rm`, but the diagnostic gap is
confusing for humans and risky for agents because the forest metadata says one
thing while Git's worktree registry says another.

## Real incident

While cleaning the OpenCOP forest `sdt2y-release-tags`, the metadata for the
Java repo recorded the expected forest-created branch:

```toml
[[repos]]
name = "opencop-java"
branch = "dliv/sdt2y-release-tags"
base_branch = "release/0.2.0-2026-06-05"
branch_created = true
```

The actual worktree registration showed the forest's Java worktree on `main`:

```text
worktree /Users/dliv/oaw/.forests/sdt2y-release-tags/opencop-java
HEAD ...
branch refs/heads/main
```

That prevented the primary Java checkout from switching to `main`:

```text
cd /Users/dliv/oaw/opencop-java
git checkout main
fatal: 'main' is already used by worktree at '/Users/dliv/oaw/.forests/sdt2y-release-tags/opencop-java'
```

`git forest rm sdt2y-release-tags` recovered cleanly:

- removed the forest worktrees
- deleted the metadata-recorded branch `dliv/sdt2y-release-tags`
- removed the forest directory
- freed `main` so the primary checkout could switch to it

This does not prove `git forest new` created the forest incorrectly. The
stronger evidence is that the forest was created with
`dliv/sdt2y-release-tags`, then a human or agent later checked out `main` inside
the forest worktree. The bug is that git-forest does not make that drift visible
before it causes a confusing branch-lock failure elsewhere.

## Current behavior

`src/commands/ls.rs` builds `branch_summary` from `RepoMeta.branch`, not from
the branch currently checked out in each repo worktree.

`src/commands/status.rs` runs `git status -sb` for each worktree and returns the
raw output, but does not compare that output or any symbolic ref with
`RepoMeta.branch`.

`src/commands/rm.rs` plans deletion from metadata and recently gained stronger
branch-deletion dry-run checks, but it still does not appear to report when the
worktree being removed is currently on a different branch than the one in
metadata.

## Expected behavior

Commands that inspect or remove an existing forest should detect branch drift
when a repo worktree exists and its checked-out branch differs from
`RepoMeta.branch`.

Reasonable behavior:

- `git forest status` reports both expected and actual branch when they differ.
- `git forest status --json` includes structured fields such as
  `expected_branch`, `actual_branch`, and `branch_drift`.
- `git forest ls` either uses actual branch information or adds a warning/count
  when a forest has drifted repos.
- `git forest rm --dry-run --json` reports the drift before mutation, especially
  if the actual branch is a shared/base branch like `main`, `dev`, or
  `release/*`.
- Actual `git forest rm` may still proceed when the worktree is clean and branch
  deletion is otherwise safe, but its human output should make clear that it is
  removing a worktree currently checked out to a branch different from the
  metadata branch.

## Implementation sketch

For each existing repo worktree, read the actual checked-out branch before
formatting status or planning removal:

```text
git -C <worktree-path> symbolic-ref --quiet --short HEAD
```

If that command fails, fall back to detached-HEAD diagnostics such as:

```text
git -C <worktree-path> rev-parse --short HEAD
```

Compare the actual branch to `RepoMeta.branch`.

For `rm`, keep the distinction between:

- the actual branch currently checked out in the worktree being removed
- the metadata branch that git-forest may delete because it created it

The branch deletion safety logic should remain based on the metadata branch,
but the user-visible plan/result should not hide that the worktree itself had
drifted to another branch.

## Test idea

1. Create a forest whose repo metadata records a forest-created branch such as
   `dliv/test-forest`.
2. Inside one forest repo worktree, manually run `git checkout main`.
3. Run `git forest status --json`.
4. Run `git forest rm <name> --dry-run --json`.

Expected:

- status reports branch drift with expected branch `dliv/test-forest` and
  actual branch `main`
- rm dry-run reports the same drift before any mutation
- actual rm still removes the worktree and deletes the forest-created metadata
  branch if the existing safety checks allow it

Also test detached HEAD:

1. Check out a commit directly inside a forest repo worktree.
2. Confirm status/rm report a detached actual state instead of silently using
   only `RepoMeta.branch`.

## Notes

This is mainly a visibility and safety-contract bug. Git itself is behaving
correctly when it refuses to check out a branch that is already active in
another worktree. The confusing part is that the forest-level commands can make
an old forest look like it owns only its recorded feature branch while it is
actually locking a shared branch.
