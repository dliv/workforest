# Phase 3 Plan — `new <name>`

## Goal

Implement `git forest new <name>` as a flag-driven, non-interactive command that creates a forest directory with git worktrees for every configured repo. Uses the plan/execute pattern (Decision 9) with `--dry-run` support.

---

## Scope Decision: Combine 3a + 3b (flag-driven only)

The architecture doc splits Phase 3 into 3a (minimal happy path), 3b (mode + exceptions), and 3c (polish). This plan combines 3a and 3b into a single implementation, covering the full flag-driven interface. Rationale:

1. **3a without exceptions is half-useful.** Feature mode works (all repos get the template branch), but review mode without per-repo overrides gives every repo a placeholder branch — there's no way to specify which repo is being reviewed.

2. **The flag-driven interface is the primary consumer.** Agents need `--mode`, `--repo-branch`, and `--dry-run` from day one. Implementing these together is more efficient than shipping a mode that can't express the most common workflow (review with one exception).

3. **The incremental complexity is small.** Per-repo overrides (`--repo-branch`) are a simple map lookup during planning. Branch resolution (local → remote → new) is needed even for feature mode (the branch might already exist). Fetch is one git call per repo.

**Not included:** interactive prompts (Phase 5), advanced error recovery for "branch checked out in another worktree" (3c polish).

---

## CLI

```
git forest new <name>
    --mode feature|review        Mode (required — interactive fallback deferred to Phase 5)
    --branch <name>              Override default branch for ALL repos
    --repo-branch <repo>=<br>    Per-repo branch override (repeatable)
    --no-fetch                   Skip fetching remotes (default: fetch)
    --dry-run                    Show plan without executing
```

`--mode` uses clap `ValueEnum` on `ForestMode` for case-insensitive parsing, auto-generated `--help` values, and consistent error messages — no manual string matching.

### Minimal invocations

```sh
# Feature: all repos get branch dliv/java-84-refactor-auth off their base_branch
git forest new java-84/refactor-auth --mode feature

# Review: all repos get forest/review-sues-dialog, except foo-web
git forest new review-sues-dialog --mode review \
  --repo-branch foo-web=sue/gh-100/fix-dialog

# Dry run: show what would happen
git forest new my-feature --mode feature --dry-run

# Agent: structured output
git forest new my-feature --mode feature --json
```

### Branch defaults by mode

| Mode    | Default branch pattern | `branch_created` |
|---------|------------------------|-------------------|
| Feature | `{user}/{name}` from `branch_template` | `true` (new branch off `{remote}/{base_branch}`) |
| Review  | `forest/{forest-name}` | `true` (new branch off `{remote}/{base_branch}`) |

The `--branch` flag overrides the default for all repos. `--repo-branch` overrides specific repos. Overridden branches go through branch resolution (may be existing, see below).

---

## Architecture

### Overall flow

```
CLI flags
    │
    ▼
NewInputs (struct)
    │
    ├── fetch_repos() ──── always unless --no-fetch (even during --dry-run)
    │
    ▼
plan_forest(inputs, config) -> Result<ForestPlan>     ← read-only git queries + pure planning
    │
    ├── --dry-run: convert plan to NewResult, return (no execution)
    │
    ▼
execute_plan(plan) -> Result<NewResult>                ← impure: mkdir, git worktree add, write meta
    │
    ▼
NewResult (struct) → main.rs formats as human or JSON
```

### Fetch behavior (decided)

Always fetch unless `--no-fetch`, **including during `--dry-run`**. Fetch is non-destructive (only updates tracking refs) and makes the plan accurate. `--no-fetch` is the explicit opt-out for offline use or speed.

### Dry-run output (decided)

Reuse `NewResult` for both dry-run and executed runs. Set `dry_run: true` and populate `repos` from the plan. Add a `checkout_kind` field to `NewRepoResult` so JSON consumers can see the resolution detail. This avoids maintaining two JSON schemas and matches the existing pattern (one result type per command).

### Key types

```rust
// --- Input ---

pub struct NewInputs {
    pub name: String,                        // forest name (e.g., "java-84/refactor-auth")
    pub mode: ForestMode,                    // Feature or Review
    pub branch_override: Option<String>,     // --branch: override for all repos
    pub repo_branches: Vec<(String, String)>,// --repo-branch: per-repo overrides
    pub no_fetch: bool,                      // --no-fetch
    pub dry_run: bool,                       // --dry-run
}

// --- Plan ---

pub struct ForestPlan {
    pub forest_name: String,
    pub forest_dir: PathBuf,
    pub mode: ForestMode,
    pub repo_plans: Vec<RepoPlan>,
}

pub struct RepoPlan {
    pub name: String,          // repo name (= directory name inside forest)
    pub source: PathBuf,       // path to source git repo
    pub dest: PathBuf,         // path to worktree (forest_dir/name)
    pub branch: String,        // target branch name
    pub base_branch: String,   // for meta recording
    pub remote: String,        // remote name (usually "origin")
    pub checkout: CheckoutKind,
}

#[derive(Debug, Clone, Serialize)]
pub enum CheckoutKind {
    /// Branch exists locally. `git worktree add <dest> <branch>`
    ExistingLocal,
    /// Branch exists on remote. `git worktree add <dest> -b <branch> <remote>/<branch>`
    TrackRemote,
    /// Branch doesn't exist. `git worktree add <dest> -b <branch> <remote>/<base_branch>`
    NewBranch,
}

// --- Result ---

#[derive(Debug, Serialize)]
pub struct NewResult {
    pub forest_name: String,
    pub forest_dir: PathBuf,
    pub mode: ForestMode,
    pub dry_run: bool,
    pub repos: Vec<NewRepoResult>,
}

#[derive(Debug, Serialize)]
pub struct NewRepoResult {
    pub name: String,
    pub branch: String,
    pub base_branch: String,
    pub branch_created: bool,
    pub checkout_kind: CheckoutKind, // for JSON consumers (especially --dry-run)
    pub worktree_path: PathBuf,
}
```

`branch_created` is derived from `CheckoutKind` (decided — this mapping is final):
- `ExistingLocal` → `false` (branch already existed)
- `TrackRemote` → `false` (branch exists on remote; local tracking branch is not ours to delete)
- `NewBranch` → `true` (we created this branch; `rm` should delete it)

---

## Branch Resolution

For a given repo with target branch `B`, remote `R`, and base branch `base`:

```
1. git show-ref --verify refs/heads/B
   → exists: CheckoutKind::ExistingLocal

2. git show-ref --verify refs/remotes/R/B
   → exists: CheckoutKind::TrackRemote

3. Neither exists:
   → CheckoutKind::NewBranch (off R/base)
```

This is called during `plan_forest()` and maps directly to git commands during execution.

### Resolution happens in the source repo

All `git show-ref` calls run in the source repo (e.g., `~/src/foo-api`), not in the worktree destination (which doesn't exist yet).

### When resolution needs a fetch

Branch resolution depends on remote tracking refs being current. If `--no-fetch` is set, resolution uses stale local state. This is documented behavior — the user opted out of freshness.

### Reject ambiguous branch inputs

Branch names starting with `refs/` or `<remote>/` (matching the configured remote for that repo) are rejected during validation with a helpful error:

```
error: branch name "origin/feature-x" looks like a remote ref
  hint: pass the branch name without the remote prefix: "feature-x"
```

This prevents misresolution where `refs/heads/origin/foo` would be checked instead of `refs/remotes/origin/foo`.

---

## Git Operations

### Fetch (pre-planning)

For each source repo, run in the source repo directory:

```
git fetch <remote>
```

Maps to: `git(&source, &["fetch", &remote])`

Always runs unless `--no-fetch`. Runs even during `--dry-run` to ensure accurate plan.

### Worktree creation (execution)

| CheckoutKind | Git command |
|---|---|
| `ExistingLocal` | `git worktree add <dest> <branch>` |
| `TrackRemote` | `git worktree add <dest> -b <branch> <remote>/<branch>` |
| `NewBranch` | `git worktree add <dest> -b <branch> <remote>/<base_branch>` |

All worktree commands run with `current_dir` set to the source repo. The `<dest>` is an absolute path (`forest_dir/repo_name`).

Maps to: `git(&source, &["worktree", "add", ...])` using the existing `git()` helper.

### New git helper

Add to `git.rs`:

```rust
/// Check if a ref exists. Returns true if `git show-ref --verify <refname>` succeeds.
pub fn ref_exists(repo: &Path, refname: &str) -> Result<bool>
```

This wraps `git show-ref --verify` and returns `Ok(true)` on exit 0, `Ok(false)` on exit 1 (ref not found), and `Err` on other failures.

---

## Incremental Meta Writing (Decision 5)

The `.forest-meta.toml` is written incrementally so `rm` can always clean up a partial forest:

1. Create forest directory.
2. Write initial meta with empty `repos: vec![]`.
3. For each repo:
   a. Run `git worktree add`.
   b. Push `RepoMeta` to `meta.repos`.
   c. Re-write meta file via `ForestMeta::write()`.
4. On failure: stop. Meta contains all successfully-created repos.

This uses the existing `ForestMeta::write()` method (re-serializes the full struct each time — cost is negligible for ~5-10 repos).

---

## Validation (in `plan_forest`)

Before planning worktree operations, validate:

1. **Forest name not empty, not `.`, not `..`.** Also reject names that sanitize to empty.
2. **Forest directory doesn't already exist.** Check both `forest_dir(worktree_base, name)` and scan for meta files with the same name (collision between original and sanitized names — Decision 2).
3. **Config has repos.** Error if `config.repos` is empty.
4. **All `--repo-branch` names match config repos.** Error with "unknown repo: X, known repos: A, B, C" on mismatch.
5. **No duplicate `--repo-branch` keys.** Error on `--repo-branch foo=a --repo-branch foo=b` — ambiguous, fail with "duplicate repo-branch for: foo".
6. **Source repos exist.** Each `config.repos[i].path` must be a directory. (Git-repo validation happened at `init` time; paths could have been moved since.)
7. **Branch names are not ambiguous.** Reject names starting with `refs/` or matching `<remote>/...` for the repo's configured remote. Error with hint to remove the prefix.
8. **Base branch ref exists on remote.** After fetch, verify `refs/remotes/<remote>/<base_branch>` exists for each repo that needs `NewBranch` checkout. Error with:
   ```
   error: origin/dev not found in foo-api
     hint: check that base_branch "dev" exists on remote "origin", or run `git fetch origin` in ~/src/foo-api
   ```

---

## Implementation Steps

### Step 0 — Fix `git init -b main` in test infrastructure

`TestEnv::create_repo()` currently runs `git init` which creates `master` on most systems, but `default_config()` sets `base_branch: "main"`. Phase 3 tests will fail when branch resolution looks for `origin/main`.

Fix: change `create_repo()` and `create_repo_with_branch()` to use `git init -b main`. Apply project-wide — this affects existing tests too, not just new ones.

### Step 1 — Add `ref_exists` helper to `git.rs`

Small, testable addition. Used by branch resolution in step 3.

```rust
pub fn ref_exists(repo: &Path, refname: &str) -> Result<bool>
```

Tests: local branch exists, local branch missing, remote ref exists, remote ref missing, invalid repo path.

### Step 2 — Expand `New` CLI variant in `cli.rs`

Add flags: `--mode`, `--branch`, `--repo-branch`, `--no-fetch`, `--dry-run`.

Derive `clap::ValueEnum` on `ForestMode` (in `meta.rs`) for `--mode`. This gives case-insensitive parsing and auto-generated help values. `ForestMode` already has `Feature` and `Review` variants — adding `ValueEnum` is additive.

```rust
// meta.rs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ForestMode {
    Feature,
    Review,
}

// cli.rs
New {
    /// Forest name
    name: String,
    /// Mode: feature or review
    #[arg(long)]
    mode: ForestMode,
    /// Override default branch for all repos
    #[arg(long)]
    branch: Option<String>,
    /// Per-repo branch override (format: repo-name=branch, repeatable)
    #[arg(long = "repo-branch")]
    repo_branches: Vec<String>,
    /// Skip fetching remotes before creating
    #[arg(long)]
    no_fetch: bool,
    /// Show plan without executing
    #[arg(long)]
    dry_run: bool,
}
```

No manual string parsing for mode — clap handles it with `ValueEnum`.

### Step 3 — Add planning types and `plan_forest()` to `commands.rs`

This is the core logic. Add:

- `NewInputs`, `ForestPlan`, `RepoPlan`, `CheckoutKind` structs
- `plan_forest(inputs: &NewInputs, config: &ResolvedConfig) -> Result<ForestPlan>`

`plan_forest` does:
1. Validate inputs (forest name, repo-branch keys, branch name guards, source paths).
2. Compute `forest_dir` from `worktree_base` + sanitized name.
3. `create_dir_all(worktree_base)` if it doesn't exist (match `discover_forests` leniency).
4. Check for directory/name collision.
5. For each repo in config:
   a. Determine target branch (from mode default, `--branch` override, or `--repo-branch` override).
   b. Resolve branch via `ref_exists()` → `CheckoutKind`.
   c. If `NewBranch`, verify `refs/remotes/<remote>/<base_branch>` exists.
   d. Build `RepoPlan`.
6. Return `ForestPlan`.

Branch computation helper:

```rust
fn compute_target_branch(
    repo_name: &str,
    forest_name: &str,
    mode: &ForestMode,
    branch_template: &str,
    username: &str,
    branch_override: &Option<String>,
    repo_branches: &[(String, String)],
) -> String
```

### Step 4 — Add `execute_plan()` and `cmd_new()` to `commands.rs`

- `execute_plan(plan: &ForestPlan) -> Result<NewResult>` — creates dirs, runs git, writes meta incrementally.
- `cmd_new(inputs: NewInputs, config: &ResolvedConfig) -> Result<NewResult>` — orchestrates fetch → plan → execute (or dry-run → convert plan to result).
- `format_new_human(result: &NewResult) -> String` — human-readable output.

Execution sequence for each repo:
```rust
match &repo_plan.checkout {
    CheckoutKind::ExistingLocal => {
        git(&source, &["worktree", "add", dest_str, &branch])?;
    }
    CheckoutKind::TrackRemote => {
        let start = format!("{}/{}", remote, branch);
        git(&source, &["worktree", "add", dest_str, "-b", &branch, &start])?;
    }
    CheckoutKind::NewBranch => {
        let start = format!("{}/{}", remote, base_branch);
        git(&source, &["worktree", "add", dest_str, "-b", &branch, &start])?;
    }
}
```

### Step 5 — Wire up in `main.rs`

Replace the `New` stub with:
1. Load config.
2. Parse `--repo-branch` strings to `(String, String)` tuples (split on first `=`).
3. Build `NewInputs` (mode comes directly from clap as `ForestMode`).
4. Call `cmd_new()` (which handles fetch, plan, execute internally).
5. Output result via `output()` helper.

### Step 6 — Tests

See Tests section below.

---

## Files Changed

| File | Changes |
|------|---------|
| `meta.rs` | Add `clap::ValueEnum` derive to `ForestMode` |
| `git.rs` | Add `ref_exists()` helper |
| `cli.rs` | Expand `New` variant with `--mode` (as `ForestMode`), `--branch`, `--repo-branch`, `--no-fetch`, `--dry-run` |
| `commands.rs` | Add `NewInputs`, `ForestPlan`, `RepoPlan`, `CheckoutKind`, `NewResult`, `NewRepoResult`; add `plan_forest()`, `execute_plan()`, `cmd_new()`, `format_new_human()` |
| `main.rs` | Wire up `New` command, parse `--repo-branch` strings |
| `config.rs` | No changes |
| `paths.rs` | No changes (existing `sanitize_forest_name`, `forest_dir` already sufficient) |
| `testutil.rs` | Fix `create_repo` to use `git init -b main`; add `create_repo_with_remote()` helper |

---

## Tests

### Step 0 — Fix existing test infra

- `create_repo` / `create_repo_with_branch` → `git init -b main`
- Verify all existing tests still pass after this change.

### Unit tests — `git.rs`

- `ref_exists_local_branch` — returns true for existing local branch
- `ref_exists_local_branch_missing` — returns false for non-existent branch
- `ref_exists_remote_ref` — returns true for existing remote ref (needs `create_repo_with_remote`)
- `ref_exists_remote_ref_missing` — returns false for non-existent remote ref

### Unit tests — `commands.rs` (plan_forest)

**Branch computation:**
- `feature_mode_uses_branch_template` — feature mode produces `{user}/{name}` branch
- `review_mode_uses_forest_prefix` — review mode produces `forest/{forest-name}` branch
- `branch_override_applies_to_all_repos` — `--branch` overrides template for every repo
- `repo_branch_override_applies_to_specific_repo` — `--repo-branch foo=bar` only affects foo
- `repo_branch_override_unknown_repo_errors` — error with hint listing valid repo names
- `duplicate_repo_branch_errors` — `--repo-branch foo=a --repo-branch foo=b` → error

**Input validation:**
- `plan_empty_name_errors` — empty forest name → error
- `plan_dot_name_errors` — `"."` or `".."` → error
- `plan_forest_dir_collision_errors` — existing directory at target path → error
- `plan_empty_config_repos_errors` — no repos configured → error
- `plan_source_repo_missing_errors` — source repo path doesn't exist → error
- `plan_ambiguous_branch_refs_prefix_errors` — branch starting with `refs/` → error
- `plan_ambiguous_branch_remote_prefix_errors` — branch starting with `origin/` → error
- `plan_base_branch_ref_missing_errors` — `origin/dev` doesn't exist → clear error with hint

**Branch resolution:**
- `plan_resolves_existing_local_branch` — existing local → `ExistingLocal`, `branch_created = false`
- `plan_resolves_remote_branch` — exists on remote → `TrackRemote`, `branch_created = false`
- `plan_resolves_new_branch` — neither exists → `NewBranch`, `branch_created = true`

**Full plan shape:**
- `plan_feature_mode_all_repos` — verify complete plan structure for feature mode
- `plan_review_mode_with_exception` — verify review mode with one `--repo-branch` override

### Unit tests — `commands.rs` (execute_plan)

- `execute_creates_forest_dir_and_worktrees` — verify directory structure after execution
- `execute_writes_meta_incrementally` — verify meta exists after partial execution (simulate failure)
- `execute_meta_matches_plan` — verify meta content matches plan's branch/base_branch/branch_created

### Integration tests — `tests/cli_test.rs`

- `new_feature_mode_creates_forest` — full end-to-end: init config, create repos, run `new`, verify worktrees exist
- `new_review_mode_with_repo_branch` — review mode with `--repo-branch`, verify branches
- `new_dry_run_does_not_create` — `--dry-run` prints plan, forest dir does not exist after
- `new_json_output` — `--json` returns valid JSON with expected fields
- `new_without_mode_errors` — missing `--mode` → clap error
- `new_without_config_errors` — no config file → error mentioning `git forest init`
- `new_duplicate_forest_name_errors` — forest dir already exists → error
- `new_no_fetch_skips_fetch` — `--no-fetch` doesn't fail when remote is unreachable
- `ls_shows_new_forest` — after `new`, `ls` shows the created forest

Update existing test:
- `subcommand_new_recognized` — update (currently expects "not yet implemented"; now requires `--mode`)

### Test infrastructure — `testutil.rs`

Add `create_repo_with_remote()`:

```rust
/// Creates a bare repo + a regular repo with the bare as `origin`.
/// Runs `git fetch origin` after setup so remote-tracking refs exist.
/// Returns the path to the regular (non-bare) repo.
pub fn create_repo_with_remote(&self, name: &str) -> PathBuf {
    // 1. Create bare repo at self.dir/bare/{name}.git via `git init --bare -b main`
    // 2. Create regular repo at self.dir/src/{name} via `git init -b main`
    // 3. git remote add origin <bare-path>
    // 4. git commit --allow-empty -m "initial"
    // 5. git push origin main
    // 6. git fetch origin   ← ensures refs/remotes/origin/main exists
    // Returns regular repo path
}
```

To test `CheckoutKind::TrackRemote` (branch exists on remote but not locally):
1. After `create_repo_with_remote`, create a branch in a temporary second clone and push it.
2. In the source repo, `git fetch origin` — now `refs/remotes/origin/<branch>` exists but `refs/heads/<branch>` does not.

Or simpler: push a branch to the bare repo directly (`git branch <name> HEAD` in the bare repo), then fetch from source.

---

## Open Questions

1. ~~Should `--dry-run` still fetch?~~ **Decided: yes.** Always fetch unless `--no-fetch`. Fetch is non-destructive and makes the plan accurate.

2. ~~`--mode` case sensitivity?~~ **Decided: use `ValueEnum`.** Clap handles case-insensitive parsing automatically.

3. **Upstream tracking.** When creating a new branch off `origin/dev`, should we set `--track`? `git worktree add -b <branch> <remote>/<base>` does NOT set upstream tracking by default. Recommendation: defer to Phase 5 — `git push -u` at push time is the more common workflow.

4. **Error on branch already checked out in another worktree.** `git worktree add` will fail with a clear message if the branch is checked out elsewhere. For Phase 3, we let git's error propagate. Phase 3c polish would detect this upfront during planning and give a better message (e.g., "branch X is already checked out in /path/to/other/worktree").

5. ~~Should `ForestPlan` be `Serialize`?~~ **Decided: no.** Reuse `NewResult` for both dry-run and real runs with `checkout_kind` field for detail. One JSON schema per command.

---

## Out of Scope (deferred)

- Interactive prompts for mode selection and exceptions → Phase 5
- `--force` to overwrite existing forest → Phase 5
- Parallel fetch across repos → Future
- Better error for "branch checked out in another worktree" → Phase 3c
- Upstream tracking setup → Phase 5
- Resuming a partially-created forest → Phase 5
- Splitting `commands.rs` into per-command modules → after Phase 3 lands (file is ~950 lines now, will grow; not blocking)
