# git-forest Bug: reset leaves forest-created branches behind

## Summary

`git forest reset --confirm` removes discovered forest worktrees and deletes the
forest directory, but it does not delete local branches that git-forest created
for those worktrees.

After reset, the worktree registration is gone and the forest directory is gone,
but the forest-created branch remains as an orphaned local ref with no
`worktreepath`.

Observed on current `main` at `55c6f1a` (`git-forest 0.4.2` source).

## Why this matters

`git forest rm <name>` treats branches recorded with `branch_created = true` as
owned by the forest and deletes them after the worktree is removed, subject to
its branch-safety checks.

`git forest reset` is broader and more destructive, so users reasonably expect
it to leave less git-forest state behind than `rm`, not more. Leaving the
branches behind creates stale local refs such as `forest/reset-repro` that no
longer correspond to any forest in `git forest ls`.

Over time, this can accumulate stale `forest/*` branches even though the forest
directories and Git worktree pointers were cleaned up.

## Isolated repro

This repro used a disposable temp directory and isolated environment variables:

```text
HOME=<lab>/home
XDG_CONFIG_HOME=<lab>/config
XDG_STATE_HOME=<lab>/state
GIT_CONFIG_GLOBAL=<lab>/gitconfig
```

The toy source repo was `api`, with a local bare `origin` and `origin/main`.

Commands:

```bash
git forest init \
  --worktree-base <lab>/forests \
  --base-branch main \
  --feature-branch-template 'feature/{name}' \
  --repo <lab>/source/api

git forest new reset-repro --mode review --no-fetch
```

Before reset, the forest-created branch is checked out in the toy forest
worktree:

```text
$ git -C <lab>/source/api for-each-ref \
    --format='%(refname:short)%09%(worktreepath)' refs/heads/forest
forest/reset-repro    <lab>/forests/reset-repro/api
```

The reset dry-run only reports config/state/forest directory removal. It does
not mention branch deletion:

```json
{
  "dry_run": true,
  "config_only": false,
  "forests": [
    {
      "name": "reset-repro",
      "path": "<lab>/forests/reset-repro",
      "removed": true
    }
  ],
  "warnings": [],
  "errors": []
}
```

Actual reset succeeds:

```bash
git forest reset --confirm --json
```

After reset, the forest directory is gone and the extra worktree registration is
gone, but the forest-created branch still exists:

```text
$ test ! -e <lab>/forests/reset-repro && echo removed
removed

$ git -C <lab>/source/api worktree list --porcelain
worktree <lab>/source/api
HEAD <initial-commit>
branch refs/heads/main

$ git -C <lab>/source/api for-each-ref \
    --format='%(refname:short)%09%(worktreepath)' refs/heads/forest
forest/reset-repro
```

The blank `worktreepath` means the branch is now just an orphaned local branch
ref, not an active worktree branch.

## Current code path

`src/commands/reset.rs` plans reset forests from `.forest-meta.toml` and records
each repo's source path and worktree directory:

```rust
struct ForestRepoInfo {
    source: AbsolutePath,
    worktree_dir: PathBuf,
}
```

During execution, reset removes each worktree registration with:

```rust
git worktree remove --force <worktree-dir>
```

Then it removes the forest directory with `remove_dir_all`.

Unlike `rm.rs`, reset does not keep the metadata branch name or
`branch_created` flag in its plan, and it never attempts to delete
forest-created branches.

## Expected behavior

`git forest reset` should clean up git-forest-created branches, or at least
report that it is intentionally leaving them behind.

Preferred behavior:

- Add `branch` and `branch_created` to reset's per-repo plan.
- After a worktree is removed, delete branches where `branch_created = true`.
- Reuse the same safety model as `git forest rm` where practical.
- Include branch deletion results in `reset --json` and human output.
- Include branch deletion in `reset --dry-run --json`, so users and agents know
  what reset will do.

If reset intentionally should not delete branches, then the command should make
that explicit in output and documentation because the behavior differs from
`git forest rm`.

## Test idea

Add an integration-style reset test:

1. Create a toy repo with `origin/main`.
2. Configure a temp git-forest environment.
3. Create a review forest named `reset-repro`.
4. Assert `refs/heads/forest/reset-repro` exists and has a worktree path.
5. Run `git forest reset --confirm`.
6. Assert the forest directory is removed.
7. Assert the worktree registration is removed.
8. Assert `refs/heads/forest/reset-repro` no longer exists, or assert/reset
   output explicitly reports the branch was intentionally retained.

Also test `reset --dry-run --json` includes planned branch deletion or retention
for forest-created branches.

## Notes

This is separate from stale Git worktree pointers. Reset does clean up the toy
worktree registration in this repro. The leftover is a normal local branch ref
that used to be checked out by the removed forest worktree.
