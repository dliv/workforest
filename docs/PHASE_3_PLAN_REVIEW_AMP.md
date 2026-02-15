# Phase 3 Plan — Review Feedback

Review of `PHASE_3_PLAN.md` against the existing codebase and architecture decisions. Apply these changes before or during implementation.

---

## Must-Fix

### 1. Use clap `ValueEnum` for `--mode`

The plan proposes `mode: String` with manual parsing in `main.rs`. The codebase uses clap derive throughout — use `ValueEnum` on `ForestMode` (or a CLI-specific wrapper) instead. This gives you case-insensitive parsing, auto-generated `--help` values, and consistent error messages without manual `eprintln!/exit`.

```rust
// Either derive ValueEnum on ForestMode directly (if it lives in cli.rs or is re-exported),
// or create a small wrapper type that converts.
#[derive(clap::ValueEnum, ...)]
pub enum ForestMode {
    Feature,
    Review,
}
```

### 2. Fix test default branch (`master` vs `main`)

`TestEnv::create_repo()` runs `git init` which creates `master` on most systems. But `default_config()` sets `base_branch: "main"`. Phase 3 tests will fail when branch resolution looks for `origin/main`.

Fix: change `create_repo` (and the new `create_repo_with_remote`) to use `git init -b main`.

This affects existing tests too — fix it project-wide, not just for new tests.

### 3. Handle duplicate `--repo-branch`

The plan doesn't define behavior for `--repo-branch foo=a --repo-branch foo=b`. Error on duplicates — it's simplest, predictable, and matches the project's "actionable error messages" principle. Test for it.

### 4. Validate base branch refs during planning

If `refs/remotes/<remote>/<base_branch>` doesn't exist, `git worktree add -b <branch> <remote>/<base>` fails with a confusing git error. Check during `plan_forest()` (after fetch) and produce a clear error:

```
error: origin/dev not found in foo-api
  hint: check that base_branch "dev" exists on remote "origin", or run `git fetch origin` in ~/src/foo-api
```

### 5. Reject ambiguous branch inputs

Architecture doc calls out that users may pass `origin/foo` or `refs/heads/foo`. The plan's branch resolution would mishandle these (e.g., checking `refs/heads/origin/foo`).

Minimal guard: reject branch names starting with `refs/` or `<remote>/` with a helpful error:

```
error: branch name "origin/feature-x" looks like a remote ref
  hint: pass the branch name without the remote prefix: "feature-x"
```

### 6. Ensure `create_repo_with_remote()` produces valid remote-tracking refs

After `git remote add origin <bare>` + `git push origin HEAD`, the local repo may NOT have `refs/remotes/origin/<branch>` (push doesn't update tracking refs the same way fetch does). The helper must run `git fetch origin` after setup, or `ref_exists(..., "refs/remotes/origin/...")` will return false in tests.

Also: to test `CheckoutKind::TrackRemote`, you need a branch that exists on the remote but NOT locally. Suggested setup:
1. Create bare repo.
2. Clone to `source`, commit on `main`, push.
3. Create branch `feature-x` on a second clone (or in bare directly), push.
4. In `source`, `git fetch origin` — now `refs/remotes/origin/feature-x` exists but `refs/heads/feature-x` does not.

---

## Design Decisions to Lock In

### 7. Fetch behavior: always fetch unless `--no-fetch`

Fetch is non-destructive (only updates tracking refs). Always fetch regardless of `--dry-run` so the plan is accurate. `--no-fetch` is the explicit opt-out.

Resolve Open Question #1 accordingly.

### 8. Dry-run output: reuse `NewResult`, don't serialize `ForestPlan`

Keep one output type for both dry-run and executed runs. Set `dry_run: true` on `NewResult` and populate `repos` from the plan. Optionally add a `checkout_kind` field to `NewRepoResult` for JSON consumers who want the detail.

This avoids maintaining two JSON schemas and matches the existing pattern (every command returns one result type). Resolve Open Question #5 accordingly.

### 9. `branch_created` for `TrackRemote` stays `false`

The plan's conservative mapping is correct:
- `ExistingLocal` → `false`
- `TrackRemote` → `false`
- `NewBranch` → `true`

`TrackRemote` does create a local branch, but it tracks an existing remote branch the user cares about — `rm` should not delete it. This matches Phase 4's "safe by default" posture.

---

## Minor Items

- **Forest name validation**: reject `""`, `"."`, `".."`, and names that sanitize to empty. `plan_forest` validation list (item 1) says "not empty" but should also cover these.
- **Worktree base creation**: `cmd_new` should `create_dir_all(worktree_base)` if it doesn't exist, rather than erroring. `discover_forests` already tolerates a missing base dir — `new` should be at least as lenient.
- **`commands.rs` is getting large** (~950 lines after Phase 3). Not blocking, but consider splitting into `commands/` module with per-command files after Phase 3 lands. Don't do it during Phase 3.
