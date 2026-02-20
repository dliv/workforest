# Squash-Merge Branch Deletion Bug — Triage & Implementation Prompt

## Bug Summary

`git forest rm` uses `git branch -d` to delete feature branches. When a branch was squash-merged (GitHub/GitLab default), `-d` fails with "not fully merged" because the squash commit has a different SHA than the original commits. The worktree is removed but the branch is left behind.

## Where to Fix

The fix is in `src/commands/rm.rs`, specifically the `delete_branch()` function (line 158). Currently at line 182:

```rust
let delete_flag = if force { "-D" } else { "-d" };
```

When `force` is false and `git branch -d` fails, the function should check whether it's safe to retry with `-D`.

## Proposed Logic

The fix is merge-strategy agnostic. Don't check how the branch was merged — check whether the branch's work is already safely on the remote.

In `delete_branch()`, when `git branch -d` fails (and `force` is false), try two fallbacks in order:

### Fallback 1: Check if branch is ancestor of base_branch

```
git merge-base --is-ancestor <branch> <base_branch>
```

If true, the branch tip is fully contained in base — safe to `-D`. This catches regular merges where `-d` failed due to HEAD position in the source repo (not pointing at base_branch). Doesn't help squash merges (by definition, different SHAs).

### Fallback 2: Check upstream — no unpushed commits

This is the squash-merge fix. Uses git plumbing, no assumptions about merge strategy or remote name.

1. **Discover the configured upstream (don't hardcode `origin`):**
   ```
   git for-each-ref --format=%(upstream:short) refs/heads/<branch>
   ```
   Empty output → no upstream configured → don't auto-force-delete.

2. **Check the branch has no unpushed commits:**
   ```
   git rev-list --count <upstream>..<branch>
   ```
   If 0 → everything local is already on the remote tracking ref → safe to `git branch -D`.

### Decision flow

```
1. Try `git branch -d`
2. If fails:
   a. If `merge-base --is-ancestor branch base_branch` → `-D`
   b. Else if upstream exists and `rev-list --count upstream..branch` == 0 → `-D`
   c. Else → fail with current error + hint ("use --force")
```

### Why not hardcode "origin"

`RepoMeta` (in `src/meta.rs`) does not store a `remote` field. The config has `ResolvedRepo.remote` (defaults to `"origin"`) but it's not available at `rm` time (ADR 0012: forests are template-agnostic after creation). Using `%(upstream:short)` from `git for-each-ref` discovers the configured upstream without assuming a remote name.

### Known limitation: remote branch deleted after merge

Most teams configure GitHub/GitLab to auto-delete the remote branch after merging. This is standard workflow, not an edge case. After `git fetch --prune`, the upstream tracking ref disappears, so fallback 2 can't help — `%(upstream:short)` returns empty or the ref is gone. In this case, the user needs `--force`. This is the conservative/correct behavior: we can't verify safety without a remote ref to compare against.

The human-readable error message should hint at this: "branch may have been squash-merged; if the remote branch was deleted, use --force".

## Key Files

- `src/commands/rm.rs` — `delete_branch()` (line 158), `RepoRmPlan` (line 16), `RmOutcome` enum (line 44)
- `src/git.rs` — `git()` and `ref_exists()` helpers
- `src/meta.rs` — `RepoMeta` struct (line 34) — has `branch`, `base_branch`, `source`, `branch_created`
- `src/config.rs` — `ResolvedRepo` (line 49) has `remote` field but not available at `rm` time

## Architecture Context

- **ADR 0003 (Plan/Execute Split):** `plan_rm()` is pure, `execute_rm()` is impure. The retry logic belongs in `execute_rm()` → `delete_branch()`.
- **ADR 0009 (Best-Effort Error Accumulation):** Errors are collected in `Vec<String>`, not early-returned. If the `-D` retry also fails, push to errors.
- **ADR 0012 (Forests Are Template-Agnostic After Creation):** `rm` works from the meta file, not the config. Don't add config dependencies to `rm`.

## RmOutcome Reporting

`RmOutcome` is an enum: `Success`, `Skipped { reason }`, `Failed { error }`. The `-D` fallback is still a `Success` — the branch was deleted. But consider whether the human-formatted output should mention the fallback (e.g., "deleted (squash-merge detected, used -D)") so the user understands what happened. This could be a new variant or a field on Success — up to you, but keep it simple.

## Testing

Tests should go in `src/commands/rm.rs` (existing test section starts around line 400). Use `TestEnv` from `src/testutil.rs` for real git repos (per ADR 0007).

Test cases:
1. **Squash-merge scenario:** Create branch, push to remote, make a squash commit on the target branch, then `rm` — should succeed with `-D` fallback
2. **Unpushed commits:** Create branch with local-only commits, `rm` — should fail with current error (no fallback)
3. **No remote tracking:** Create branch that was never pushed, `rm` — should fail with current error
4. **Already force:** `rm --force` — should use `-D` directly (existing behavior, just verify it still works)

## Project Conventions

- Read `CLAUDE.md` for commit conventions, author email, and design philosophy
- Conventional commits: `fix: fall back to branch -D for squash-merged branches`
- Newtypes for validation, validate at boundaries
- Run `just check` and `just test` to verify
