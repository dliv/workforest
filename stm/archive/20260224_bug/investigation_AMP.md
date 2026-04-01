# Investigation: `git forest rm` idempotency bug

## Root Cause — Confirmed

The bug report is accurate. Two issues combine:

### 1. `delete_branch()` treats "branch not found" as a hard error (`rm.rs:280-288`)

When `force=true`, `git branch -D <branch>` is called via the `git()` wrapper (`git.rs:5-30`), which `bail!`s on _any_ non-zero exit code. Git exits non-zero when the branch doesn't exist, and this becomes an error pushed to the `errors` vec — even though the desired state (branch gone) is already achieved.

The same issue exists in the non-force path (`rm.rs:292-318`): `git branch -d` on a missing branch fails, then `can_safely_force_delete()` also fails (the ref doesn't exist for merge-base checks), so it falls through to the "not fully merged" error path.

### 2. Errors gate forest directory cleanup (`rm.rs:190-194`)

```rust
let forest_dir_removed = if errors.is_empty() {
    remove_forest_dir(&plan.forest_dir, force, &mut errors)
} else {
    false  // spurious "not found" errors block this
};
```

The accumulated "branch not found" errors prevent forest directory removal, leaving orphaned `.forest-meta.toml` that causes the forest to persist in `git forest ls`.

## Proposed Fix

**Pre-check with `ref_exists()` before attempting deletion.** Add a guard early in `delete_branch()`, after the existing `source_exists` check:

```rust
// After the source_exists check (line 278), add:
let refname = format!("refs/heads/{}", repo_plan.branch);
if !crate::git::ref_exists(&repo_plan.source, &refname).unwrap_or(false) {
    return RmOutcome::Skipped {
        reason: "branch already deleted".to_string(),
    };
}
```

This makes `rm` idempotent — if the branch is already gone, it's a skip (not an error), and errors remain empty, allowing forest directory cleanup to proceed.

**Why `ref_exists` over parsing stderr:**
- `ref_exists()` already exists in `git.rs` and is well-tested
- Avoids coupling to git's human-facing error text (varies by version and locale)
- Follows the project's "validate at boundaries" philosophy

**Optional TOCTOU guardrail:** In the error handler for `git branch -D/-d`, re-check `ref_exists()`. If the branch disappeared between the check and the delete, treat as `Skipped`. This handles the unlikely race without parsing stderr.

## Regression Test

Add a test that reproduces the exact multi-repo partial-failure scenario:

1. Create a forest with 2 repos, both with `branch_created=true`
2. Add an unpushed commit to repo A so `git branch -d` fails ("not fully merged")
3. First `cmd_rm(force=false)` — branch deletion fails for A, succeeds for B
4. Second `cmd_rm(force=true)` on re-read metadata — should:
   - Delete A's branch (force)
   - Skip B's already-deleted branch (no error)
   - Have `errors.is_empty() == true`
   - Have `forest_dir_removed == true`

## Effort Estimate

S–M (~1-2h): `ref_exists` guard + regression test + optional TOCTOU re-check.
