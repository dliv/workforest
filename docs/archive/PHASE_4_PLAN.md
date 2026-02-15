# Phase 4 Plan — `rm <name>`

## Goal

Implement `git forest rm [name]` to safely tear down forests created by `new`. Uses the plan/execute pattern (Decision 9) with `--dry-run` and `--force` support. Best-effort cleanup: continue on individual failures, report all errors at end (Decision 4).

---

## Open Questions — Resolved

### 1. Branch deletion safety

**Decided: `-d` by default, `-D` with `--force`.**

`git branch -d` (safe delete) fails if the branch has unmerged work. This is the right default — `rm` should not silently destroy unmerged commits. With `--force`, use `git branch -D` (force delete) for users who know what they're doing.

Only attempt branch deletion when `branch_created == true` in meta. Branches we didn't create (e.g., `TrackRemote`, `ExistingLocal`) are never touched.

### 2. Dirty worktree handling

**Decided: let git's error propagate; `--force` passes `--force` to git.**

`git worktree remove` fails on uncommitted changes unless `--force` is passed. Without `--force`, the error propagates as a per-repo failure — best-effort continues to the next repo. With `--force`, pass `--force` to `git worktree remove`.

No upfront detection — git's error message is already clear ("has changes, use --force to delete it").

### 3. Should rm require --force to delete branches at all?

**Decided: no.** Branch deletion (for `branch_created == true`) happens by default with `-d`. The safe delete flag is sufficient protection — it won't delete unmerged work. `--force` escalates to `-D`, not enables deletion.

### 4. CLI shape

**Decided:**

```
git forest rm [name] [--force] [--dry-run]
```

`name` is optional — auto-detect from cwd (via existing `detect_current_forest`). This matches `status`.

---

## CLI

```
git forest rm [name]
    --force      Force removal of dirty worktrees (--force to git) and unmerged branches (-D)
    --dry-run    Show what would be removed without executing
```

### Invocations

```sh
# Remove by name
git forest rm review-sues-dialog

# Remove from inside the forest (auto-detect)
cd ~/worktrees/review-sues-dialog/foo-api
git forest rm

# Preview what would be removed
git forest rm review-sues-dialog --dry-run

# Force remove dirty worktrees and unmerged branches
git forest rm review-sues-dialog --force

# Agent: structured output
git forest rm review-sues-dialog --json
```

---

## Architecture

### Overall flow

```
CLI flags
    │
    ▼
main.rs: load config, resolve forest (find dir + meta)
    │
    ▼
cmd_rm(forest_dir, meta, force, dry_run) -> Result<RmResult>
    │
    ├── plan_rm(forest_dir, meta) -> RmPlan          ← read-only: check what exists
    │
    ├── --dry-run: convert plan to RmResult, return
    │
    ▼
    execute_rm(plan, force) -> RmResult               ← impure: git worktree remove, git branch -d, fs::remove
```

### Forest resolution

Load config for `worktree_base` (needed for name-based resolution via `find_forest`). Once resolved, only the meta is used for all rm operations. This matches the `status` and `exec` pattern and is consistent with Decision 6 ("meta is self-contained" for operations, config only for discovery).

### Key types

```rust
pub struct RmPlan {
    pub forest_name: String,
    pub forest_dir: PathBuf,
    pub repo_plans: Vec<RepoRmPlan>,
}

pub struct RepoRmPlan {
    pub name: String,
    pub worktree_path: PathBuf,
    pub source: PathBuf,
    pub branch: String,
    pub branch_created: bool,
    pub worktree_exists: bool,     // checked during planning
    pub source_exists: bool,       // checked during planning
}

#[derive(Debug, Serialize)]
pub struct RmResult {
    pub forest_name: String,
    pub forest_dir: PathBuf,
    pub dry_run: bool,
    pub force: bool,
    pub repos: Vec<RepoRmResult>,
    pub forest_dir_removed: bool,
    pub errors: Vec<String>,       // all accumulated errors
}

#[derive(Debug, Serialize)]
pub struct RepoRmResult {
    pub name: String,
    pub worktree_removed: RmOutcome,
    pub branch_deleted: RmOutcome,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum RmOutcome {
    Success,
    Skipped { reason: String },
    Failed { error: String },
}
```

---

## Execution Sequence

For each repo (from meta), in order:

### 1. Remove worktree

```
if !worktree_exists:
    RmOutcome::Skipped("worktree already missing")
elif !source_exists:
    fs::remove_dir_all(worktree_path)  // best-effort, can't use git
    warn("source repo missing, removed worktree directory directly")
else:
    git worktree remove <worktree_path>           // without --force
    git worktree remove --force <worktree_path>   // with --force
```

Run `git worktree remove` from the **source repo** (where the worktree is registered).

### 2. Delete branch (if branch_created)

```
if !branch_created:
    RmOutcome::Skipped("branch not created by forest")
elif worktree_remove_failed:
    RmOutcome::Skipped("worktree still exists, cannot delete branch")
elif !source_exists:
    RmOutcome::Skipped("source repo missing")
else:
    git branch -d <branch>     // without --force
    git branch -D <branch>     // with --force
```

Run `git branch -d/-D` from the **source repo**. Skip if worktree removal failed (git won't delete a branch that's still checked out in a worktree).

### 3. Remove forest directory

After all repos are processed:

```
if force:
    fs::remove_dir_all(forest_dir)
else:
    fs::remove_file(forest_dir/.forest-meta.toml)   // remove meta first
    fs::remove_dir(forest_dir)                        // non-recursive: only succeeds if empty
```

Without `--force`, use non-recursive `remove_dir` so we don't silently delete worktree directories that failed to remove. If it fails (directory not empty), report the error — the user can see which worktrees remained.

With `--force`, use `remove_dir_all` to clean everything.

---

## Human Output

### Successful removal

```
Removed forest "review-sues-dialog"
  foo-api: worktree removed, branch forest/review-sues-dialog deleted
  foo-web: worktree removed (branch not ours)
  foo-infra: worktree removed, branch forest/review-sues-dialog deleted
Forest directory removed.
```

### Dry run

```
Dry run — no changes will be made.

Would remove forest "review-sues-dialog"
  foo-api: remove worktree, delete branch forest/review-sues-dialog
  foo-web: remove worktree (branch not ours)
  foo-infra: remove worktree, delete branch forest/review-sues-dialog
  Would remove forest directory
```

### With errors

```
Removed forest "review-sues-dialog" (with errors)
  foo-api: worktree removed, branch forest/review-sues-dialog deleted
  foo-web: worktree FAILED (has changes, use --force), branch skipped
  foo-infra: worktree removed, branch FAILED (not fully merged)

Errors:
  foo-web: git worktree remove failed: ...
  foo-infra: git branch -d failed: ...

Forest directory not removed (not empty).
  hint: resolve errors above, then rm the directory manually or re-run with --force
```

---

## Implementation Steps

### Step 1 — Expand `Rm` CLI variant in `cli.rs`

Add `--force` and `--dry-run` flags to the existing `Rm` variant.

```rust
Rm {
    /// Forest name (or auto-detect from cwd)
    name: Option<String>,
    /// Force removal of dirty worktrees and unmerged branches
    #[arg(long)]
    force: bool,
    /// Show what would be removed without executing
    #[arg(long)]
    dry_run: bool,
},
```

### Step 2 — Add rm types and `plan_rm()` to `commands.rs`

Add `RmPlan`, `RepoRmPlan`, `RmResult`, `RepoRmResult`, `RmOutcome` structs.

`plan_rm(forest_dir, meta) -> RmPlan` reads filesystem state (worktree exists? source exists?) and builds the plan. This is read-only — no mutations.

### Step 3 — Add `execute_rm()` to `commands.rs`

`execute_rm(plan, force) -> RmResult` carries out the plan:

1. For each repo: remove worktree, then conditionally delete branch.
2. Remove forest directory (meta file + dir).
3. Accumulate all errors, never bail early.

### Step 4 — Add `cmd_rm()` and `format_rm_human()` to `commands.rs`

- `cmd_rm(forest_dir, meta, force, dry_run) -> Result<RmResult>` orchestrates plan → execute (or dry-run).
- `format_rm_human(result: &RmResult) -> String` formats the human-readable output.

### Step 5 — Wire up in `main.rs`

Replace the `Rm` stub:

1. Load config.
2. Resolve forest via `resolve_forest(worktree_base, name)`.
3. Call `cmd_rm(forest_dir, meta, force, dry_run)`.
4. Output result via `output()` helper.
5. Exit with code 1 if there were errors (partial failure).

### Step 6 — Tests

See Tests section below.

---

## Files Changed

| File | Changes |
|------|---------|
| `cli.rs` | Add `--force` and `--dry-run` flags to `Rm` variant |
| `commands.rs` | Add `RmPlan`, `RepoRmPlan`, `RmResult`, `RepoRmResult`, `RmOutcome`; add `plan_rm()`, `execute_rm()`, `cmd_rm()`, `format_rm_human()` |
| `main.rs` | Wire up `Rm` command: load config, resolve forest, call `cmd_rm`, output result |
| `tests/cli_test.rs` | Add rm integration tests, update `subcommand_rm_recognized` |

No changes to `meta.rs`, `forest.rs`, `git.rs`, `config.rs`, `paths.rs`, `testutil.rs` — all required infrastructure already exists.

---

## Tests

### Unit tests — `commands.rs`

**Plan tests:**
- `plan_rm_basic` — plan has correct forest name, dir, and repo plans from meta
- `plan_rm_detects_worktree_exists` — worktree_exists is true when directory exists
- `plan_rm_detects_worktree_missing` — worktree_exists is false when directory is gone
- `plan_rm_records_branch_created` — branch_created matches meta

**Execute tests (using TestEnv + cmd_new to create real forests):**
- `rm_removes_worktrees` — after rm, worktree directories are gone
- `rm_deletes_created_branches` — branches with branch_created=true are deleted from source repos
- `rm_skips_uncreated_branches` — branches with branch_created=false are not deleted
- `rm_removes_forest_dir` — forest directory is gone after rm
- `rm_removes_meta_file` — .forest-meta.toml is gone after rm
- `rm_best_effort_continues_on_failure` — failure removing one worktree doesn't prevent removing others
- `rm_force_removes_dirty_worktree` — --force handles uncommitted changes
- `rm_force_deletes_unmerged_branch` — --force uses -D for unmerged branches
- `rm_missing_worktree_skips_gracefully` — already-deleted worktree is skipped without error
- `rm_source_repo_missing_handles_gracefully` — missing source repo doesn't crash

**cmd_rm / format tests:**
- `cmd_rm_dry_run_no_changes` — dry-run returns result but doesn't remove anything
- `cmd_rm_returns_result` — successful rm returns correct RmResult
- `format_rm_human_success` — output includes "Removed forest" and per-repo summary
- `format_rm_human_dry_run` — output includes "Dry run"
- `format_rm_human_with_errors` — output includes error details

**Round-trip test:**
- `new_then_rm_then_ls_empty` — create forest, rm it, ls shows empty

### Integration tests — `tests/cli_test.rs`

- `rm_removes_forest` — end-to-end: `new` then `rm`, verify worktrees and forest dir gone
- `rm_dry_run_preserves_forest` — `--dry-run` doesn't remove anything
- `rm_json_output` — `--json` returns valid JSON with expected fields
- `rm_nonexistent_forest_errors` — error for unknown forest name
- `rm_force_flag` — `--force` removes dirty worktree

Update existing:
- `subcommand_rm_recognized` — change expectation from "not yet implemented" to proper behavior (will fail since no config exists; update to test with proper setup or change to test error message)

### Expected test count

~16 unit + ~5 integration = ~21 new tests. Total: ~134 (113 existing + 21 new).

---

## Edge Cases Handled

| Scenario | Behavior |
|----------|----------|
| Worktree already deleted | Skip with "already missing", continue |
| Source repo moved/deleted | Remove worktree dir directly, skip branch deletion, warn |
| Branch already deleted | `git branch -d` fails, record error, continue |
| Dirty worktree (no --force) | `git worktree remove` fails, record error, skip branch, continue |
| Unmerged branch (no --force) | `git branch -d` fails, record error, continue |
| Partial forest (new failed halfway) | Meta has only successfully-created repos; rm processes only those |
| Forest dir not empty after cleanup | Report "not removed" with hint to use --force |

---

## Out of Scope (deferred)

- Interactive confirmation prompt before removal → Phase 5
- `--yes` flag to skip confirmation → Phase 5
- `git worktree prune` after removal (not needed if `git worktree remove` succeeds) → Future
- Removing a forest without config (pure cwd-based, no worktree_base lookup) → Phase 5
