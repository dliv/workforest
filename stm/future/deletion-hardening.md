# Deletion Hardening — Future Ideas

Ideas from oracle review of rollback safety in `execute_plan` and `execute_reset`.

## Current state

- `execute_plan` uses `create_dir` (not `create_dir_all`) to atomically fail if the forest dir already exists, closing the TOCTOU gap between planning and execution.
- On failure, rollback calls `git worktree remove --force` for each successfully-created worktree, then `remove_dir_all` on the forest directory.
- `execute_reset` calls `git worktree remove --force` per-repo before `remove_dir_all` on each forest.

## Potential future hardening

### 1. Created-by-this-run marker file

After creating the forest dir, write a marker file with `OpenOptions::new().create_new(true)`:

```rust
let marker = plan.forest_dir.join(".git-forest.in_progress");
let mut f = OpenOptions::new()
    .write(true)
    .create_new(true)
    .open(&marker)?;
writeln!(f, "name={}", plan.forest_name.as_str())?;
writeln!(f, "started_at={}", chrono::Utc::now().to_rfc3339())?;
```

Before `remove_dir_all` in rollback, verify the marker exists and matches. Refuse to delete if it doesn't. This proves "we created this directory" rather than trusting path derivation alone.

### 2. Constrain worktree removal targets

Before calling `git worktree remove --force` during rollback, assert:
- `repo_plan.dest.starts_with(&plan.forest_dir)` — ensures the path is inside the forest directory
- Optionally verify the worktree is registered: `git worktree list --porcelain` contains the dest path

Prevents a bug/misconfig from pointing `--force` at an arbitrary directory.

### 3. Restrictive permissions on forest dir (Unix)

If `worktree_base` could be in a shared writable location (`/tmp`, shared NFS):
- Set mode `0o700` on the forest dir after creation
- Warn/bail if `worktree_base` is world-writable without sticky bit, or not owned by current user

Standard defense against symlink/race attacks in shared directories.

### 4. Staging + rename pattern

Create `forest_dir.tmp.<nonce>` under worktree_base, build everything there, then `rename` to the final forest dir only on success. On failure, delete only the tmp dir. Avoids any risk of deleting a "real" directory. Bigger behavioral change since paths change late.

### 5. Filesystem-level locking

Use `flock`/`fcntl` on a lock file in `worktree_base` to prevent two `git forest new` runs from racing on the same name. Only relevant for CI/automation scenarios with concurrent forest creation.

## Priority

Low. The current `create_dir` atomicity covers the practical case. These ideas are for if the tool is used in shared/multi-user environments or if worktree_base misconfiguration becomes a reported issue.
