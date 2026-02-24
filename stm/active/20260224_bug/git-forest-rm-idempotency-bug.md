## Bug Report: `git forest rm` is not idempotent — "branch not found" treated as failure prevents cleanup

**Version:** git-forest 0.3.0
**Severity:** High — leaves orphaned forests that cannot be removed without manual intervention

### Summary

`git forest rm` fails to complete when run a second time after a partial failure. Already-deleted branches cause `git branch -D` to return "branch not found" (exit code 1), which the code treats as a hard error. These accumulated errors prevent the forest directory from being cleaned up, leaving an orphaned `.forest-meta.toml` that causes the forest to persist in `git forest ls` indefinitely.

### Reproduction Steps

```
# Initial state: "sidebar-links" forest across 3 repos
git forest rm sidebar-links
# Partial success: worktrees removed in all 3, branches deleted in 2/3
# foo-web branch fails with "not fully merged"

git forest rm sidebar-links --force
# EXPECTED: completes removal (deletes remaining branch, removes forest dir)
# ACTUAL: fails on the 2 already-deleted branches, forest dir left behind
```

### Root Cause

**`rm.rs:280-288` — `delete_branch()` with `force=true` does not handle "branch not found":**

```rust
if force {
    return match crate::git::git(&repo_plan.source, &["branch", "-D", &repo_plan.branch]) {
        Ok(_) => RmOutcome::Success,
        Err(e) => {
            // BUG: "branch not found" is treated as a failure,
            // but the desired state (branch gone) is already achieved
            let msg = format!("{}: git branch -D failed: {}", repo_plan.name, e);
            errors.push(msg.clone());
            RmOutcome::Failed { error: msg }
        }
    };
}
```

When `git branch -D <branch>` fails because the branch doesn't exist, the error `branch '<name>' not found` is treated identically to a real failure. The goal of deletion is that the branch should not exist — if it's already gone, that's a success.

**Cascading consequence — `rm.rs:190-194` — errors block forest directory cleanup:**

```rust
let forest_dir_removed = if errors.is_empty() {
    remove_forest_dir(&plan.forest_dir, force, &mut errors)
} else {
    // BUG: "branch not found" errors prevent this from ever running
    false
};
```

Because the "branch not found" errors accumulate, the forest directory is never cleaned up, even with `--force`.

### What Happened Step By Step

| Step | foo-java | foo-web | foo-infra |
|------|----------|---------|-----------|
| **Run 1** (no --force) | worktree removed, branch deleted | worktree removed, **branch FAILED** ("not fully merged") | worktree removed, branch deleted |
| **Run 2** (--force) | worktree skipped, **branch FAILED** ("not found") | worktree skipped, branch deleted | worktree skipped, **branch FAILED** ("not found") |
| **Final state** | clean | clean | clean |

Run 2 actually accomplished everything needed (foo-web branch deleted), but the 2 spurious "not found" errors prevent the forest directory from being removed.

### Current Machine State

| Component | State |
|-----------|-------|
| Worktrees (`~/worktrees/sidebar-links/{foo-java,web,infra}`) | All gone |
| Branches (`dliv/sidebar-links` in all 3 repos) | All gone |
| Forest metadata (`~/worktrees/sidebar-links/.forest-meta.toml`) | **Still present (orphaned)** |
| Forest directory (`~/worktrees/sidebar-links/`) | **Still present (contains only .forest-meta.toml)** |
| `git forest ls` | **Still shows `sidebar-links` as active** |

### Forest Metadata (orphaned file)

```toml
# ~/worktrees/sidebar-links/.forest-meta.toml
name = "sidebar-links"
created_at = "2026-02-24T00:46:39.577482Z"
mode = "feature"

[[repos]]
name = "foo-java"
source = "/Users/dliv/c/foo-java"
branch = "dliv/sidebar-links"
base_branch = "dev"
branch_created = true

[[repos]]
name = "foo-web"
source = "/Users/dliv/c/foo-web"
branch = "dliv/sidebar-links"
base_branch = "dev"
branch_created = true

[[repos]]
name = "foo-infra"
source = "/Users/dliv/c/foo-infra"
branch = "dliv/sidebar-links"
base_branch = "dev"
branch_created = true
```

### Suggested Fix

In `delete_branch()`, when `git branch -D` (or `git branch -d`) fails, check if stderr contains `not found`. If the branch simply doesn't exist, return `RmOutcome::Success` (or a new `Skipped` variant like "branch already deleted") instead of pushing to `errors`. This makes `rm` idempotent — running it again after a partial failure converges to a clean state.

The same "not found" check should likely apply in both the `force` path (line 280) and the non-force path (line 292), since `git branch -d` also fails with "not found" if the branch was already deleted.

### Workaround

```bash
rm ~/worktrees/sidebar-links/.forest-meta.toml
rmdir ~/worktrees/sidebar-links
```
