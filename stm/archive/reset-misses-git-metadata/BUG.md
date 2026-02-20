# Bug: `reset` deletes forest directories but leaves stale git worktree registrations

## Observed behavior

After `git forest reset --confirm`, re-creating forests with the same names fails:

```
$ git forest reset --confirm
Forests:
  Removing foo (/Users/davidlivingston/worktrees/foo)... done
  ...

$ git forest init ...
$ git forest new foo --mode feature
error: git worktree add .../foo/opencop-java dliv/foo failed (exit code: 128)
stderr: fatal: '.../foo/opencop-java' is a missing but already registered worktree;
use 'add -f' to override, or 'prune' or 'remove' to clear
```

## Root cause

`execute_reset` in `src/commands/reset.rs:129` calls `std::fs::remove_dir_all(path)` to delete forest directories. This removes the files on disk but does **not** clean up git's worktree registry (stored in each source repo's `.git/worktrees/<name>/` directory).

Compare with `rm` (`src/commands/rm.rs:164`) which correctly calls `git worktree remove` per-repo before deleting branches.

## Secondary issue: `new` doesn't clean up on failure

`execute_plan` in `src/commands/new.rs:279` calls `create_dir_all(&plan.forest_dir)` early, then proceeds to `git worktree add`. If the worktree add fails (e.g., due to stale registrations), the command bails out but leaves the partially-created forest directory behind. On retry (even after `git worktree prune`), `new` hits the `fdir.exists()` guard at line 140 and refuses to proceed.

This is how the bug compounds: reset leaves stale registrations, `new` fails on them but leaves a directory behind, and then even after pruning git, `new` still fails.

## Fix

### Reset (primary fix)

Before calling `remove_dir_all`, reset should remove each repo's worktree via git. Two options:

**Option A: Use `git worktree remove` per-repo (like `rm` does)**

This requires knowing which repos have worktrees in each forest. The forest's `.forest.json` metadata has this info. Read it before deleting the directory, then call `git worktree remove` for each repo entry.

**Option B: Call `git worktree prune` on each source repo after deleting directories**

Simpler — just delete the directories first (as today), then iterate source repos from config and run `git worktree prune`. This cleans up any dangling worktree registrations. Less precise but doesn't require reading per-forest metadata.

Option A is more correct (it also deletes branches like `rm` does). Option B is simpler and sufficient if we only care about unblocking re-creation.

### `new` (secondary fix)

`execute_plan` should clean up the forest directory if any step fails. Wrap the execution in a scope that calls `remove_dir_all(&plan.forest_dir)` on error, or at minimum remove the directory before bailing.

## Repro steps

```bash
git forest init --repo /path/to/repo-a --repo /path/to/repo-b \
  --base-branch dev --feature-branch-template "test/{name}"
git forest new foo --mode feature
git forest reset --confirm
# Re-init and retry:
git forest init ...  # same as above
git forest new foo --mode feature  # FAILS: stale worktree registration
```

## Relevant code

- `src/commands/reset.rs:129` — `remove_dir_all` without git cleanup
- `src/commands/rm.rs:163-177` — correct approach using `git worktree remove`
- `src/commands/new.rs:279` — `create_dir_all` without cleanup on failure
- `src/commands/new.rs:140` — `fdir.exists()` guard that blocks retry
