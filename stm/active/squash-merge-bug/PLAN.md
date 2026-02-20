# Fix: `git forest rm` should handle squash-merged branches

## Context

`git forest rm` uses `git branch -d` which fails for squash-merged branches ("not fully merged") because the squash commit has a different SHA than the original commits. The worktree is removed but the branch is left behind. This affects most teams since squash merge is the default on GitHub/GitLab.

## Changes

### 1. Add `base_branch` to `RepoRmPlan` (`src/commands/rm.rs:16`)

Add `base_branch: String` field so `delete_branch` can check ancestry.

### 2. Update `plan_rm()` (`src/commands/rm.rs:54`)

Populate `base_branch` from `repo.base_branch`.

### 3. Add `can_safely_force_delete()` helper (`src/commands/rm.rs`, new function)

When `git branch -d` fails and `force` is false, check two fallbacks:

- **Fallback 1:** `git merge-base --is-ancestor <branch> <base_branch>` — catches regular merges where `-d` failed due to HEAD position
- **Fallback 2:** Check upstream via `git for-each-ref --format=%(upstream:short)` then `git rev-list --count <upstream>..<branch>` == 0 — catches squash merges where branch is fully pushed

### 4. Update `delete_branch()` (`src/commands/rm.rs:158`)

On `-d` failure (when not `--force`):
- If `can_safely_force_delete()` returns true, retry with `git branch -D`
- Otherwise, fail with improved error message: hint about squash merges and `--force`

### 5. Keep `RmOutcome::Success` unchanged

The `-D` fallback is still a successful deletion. No need to change the enum — keeping it simple.

### 6. Add tests (`src/commands/rm.rs`, test section)

- **Squash-merge scenario:** Create branch, push, simulate squash-merge on base, `rm` should succeed via fallback
- **Unpushed commits:** Branch with local-only commits, `rm` should fail (no fallback)
- **No remote tracking:** Branch never pushed, `rm` should fail
- **Force still works:** `rm --force` uses `-D` directly (existing test, verify preserved)

## Verification

```
just check    # fmt + clippy
just test     # all tests including new ones
```
