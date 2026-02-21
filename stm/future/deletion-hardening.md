# Deletion Hardening — Future Ideas

Ideas from oracle review of rollback safety in `execute_plan` and `execute_reset`.

## Current state

- `execute_plan` uses `create_dir` (not `create_dir_all`) to atomically fail if the forest dir already exists, closing the TOCTOU gap between planning and execution.
- On failure, rollback calls `git worktree remove --force` for each successfully-created worktree, then `remove_dir_all` on the forest directory.
- `execute_reset` calls `git worktree remove --force` per-repo before `remove_dir_all` on each forest.

## Potential future hardening

### 1. ~~Created-by-this-run marker file~~ — Declined

**Decision:** Not needed. `create_dir` (not `create_dir_all`) already fails atomically if the directory exists, which closes the TOCTOU race the marker file was meant to guard against. A marker file doesn't add real security either — anyone who can write to `worktree_base` can write a fake marker. The `create_dir` comment in `execute_plan` documents this invariant.

### 2. ~~Constrain worktree removal targets~~ — Done

**Implemented:** Release-mode `assert!` calls added at all deletion sites:
- `rm.rs` `execute_rm`: asserts `worktree_path.starts_with(forest_dir)` before each repo removal
- `new.rs` `execute_plan` rollback: asserts `dest.starts_with(forest_dir)` before `git worktree remove --force`
- `reset.rs` `execute_reset`: asserts `worktree_dir.starts_with(forest.path)` before `git worktree remove --force`

These are release-mode (not `debug_assert!`) because a violation indicates an application bug that would cause data loss — better to panic than silently delete the wrong directory.

### 3. Restrictive permissions on forest dir (Unix)

If `worktree_base` could be in a shared writable location (`/tmp`, shared NFS):
- Set mode `0o700` on the forest dir after creation
- Warn/bail if `worktree_base` is world-writable without sticky bit, or not owned by current user

Standard defense against symlink/race attacks in shared directories.

### 4. ~~Staging + rename pattern~~ — Declined

**Decision:** The deletion safety benefit is illusory. The claimed advantage is that rollback deletes `.staging.<nonce>/` (which "must" be ours) instead of the real forest dir. But `create_dir` already guarantees ownership — if it succeeded, we created the dir, so deleting it in rollback is equally safe. The nonce adds no real information.

Meanwhile, staging has significant downsides: `git worktree add` records the exact destination path in the source repo's `.git/worktrees/<id>/gitdir`, so renaming the parent dir after creation leaves stale path registrations that break `git worktree list/remove`. And it undermines ADR-0011's crash recovery guarantee — a crash during staging leaves worktrees in a tmp dir that `discover_forests` can't find, so `rm` can't clean them up.

Staging does provide *creation* atomicity (the forest either fully appears or doesn't), but that's a different property than deletion safety, and not worth the complexity.

### 5. Filesystem-level locking

Use `flock`/`fcntl` on a lock file in `worktree_base` to prevent two `git forest new` runs from racing on the same name. Only relevant for CI/automation scenarios with concurrent forest creation.

## Priority

Low. The current `create_dir` atomicity covers the practical case. These ideas are for if the tool is used in shared/multi-user environments or if worktree_base misconfiguration becomes a reported issue.
