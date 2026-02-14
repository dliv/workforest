# Phase 1 Plan — Read-Only Commands (`ls`, `status`, `exec`)

## Goal

Wire up the three read-only commands so they work against real forests. These become the feedback loop for developing `new` in Phase 3.

## Pre-requisite: Shared plumbing

Before any command, we need two things that don't exist yet:

### 1. Default config path

A function to resolve the XDG config path (`~/.config/git-forest/config.toml`). The `directories` crate is already a dependency. Something like:

```rust
pub fn default_config_path() -> Result<PathBuf>
```

This goes in `config.rs`. Used by all commands (except `detect_current_forest` which doesn't need config).

### 2. Resolve-forest helper

`status` and `exec` both take `Option<name>` and need to resolve to `(PathBuf, ForestMeta)`. Factor this out so we don't duplicate the "if name → find_forest, else → detect_current_forest" logic.

```rust
pub fn resolve_forest(worktree_base: &Path, name: Option<&str>) -> Result<(PathBuf, ForestMeta)>
```

This goes in `forest.rs`. Returns a clear error if no forest is found (either "forest X not found" or "not inside a forest directory").

### 3. Command module structure

Extract command handlers out of `main.rs` into a `commands.rs` module (single file for now — three small functions don't need individual files). Each handler takes resolved inputs and returns `Result<()>`:

```rust
pub fn cmd_ls(worktree_base: &Path) -> Result<()>
pub fn cmd_status(forest_dir: &Path, meta: &ForestMeta) -> Result<()>
pub fn cmd_exec(forest_dir: &Path, meta: &ForestMeta, cmd: &[String]) -> Result<()>
```

`main.rs` becomes: load config → resolve args → call handler.

---

## Command: `ls`

**What it does:** List all forests with name, age, mode, and branch summary.

**Flow:**
1. Load config → `worktree_base`
2. `discover_forests(worktree_base)` → `Vec<ForestMeta>`
3. Format and print each forest

**Output format** (from architecture doc, adapted):
```
NAME                          AGE       MODE      BRANCHES
java-84-refactor-auth         2d ago    feature   dliv/java-84/refactor-auth (3 repos)
review-sues-dialog            4h ago    review    forest/review-sues-dialog (3), sue/gh-100/fix-dialog (1)
```

**Implementation notes:**
- Age formatting: compute `Utc::now() - created_at`, display as "Xm ago" / "Xh ago" / "Xd ago"
- Branch summary: collect unique branch names from `meta.repos`, show count per branch
- If no forests exist, print a helpful message ("No forests found. Create one with `git forest new <name>`")
- If config doesn't exist, error with message pointing to `git forest init`

**Tests:**
- Unit test: create temp dir with multiple forest meta files, verify output
- Edge: empty worktree_base → helpful message
- Edge: missing config → helpful error

---

## Command: `status [name]`

**What it does:** Show `git status -sb` for each repo in a forest.

**Flow:**
1. Load config → `worktree_base`
2. `resolve_forest(worktree_base, name)` → `(forest_dir, meta)`
3. For each repo in `meta.repos`:
   - Compute worktree path: `forest_dir / repo.name`
   - Run `git(&worktree_path, &["status", "-sb"])` (capture mode)
   - Print with repo name header

**Output format:**
```
=== foo-api ===
## dliv/java-84/refactor-auth...origin/dev [ahead 3]

=== foo-web ===
## dliv/java-84/refactor-auth...origin/dev
 M src/App.tsx

=== foo-infra ===
## dliv/java-84/refactor-auth...origin/dev [ahead 1]
```

**Error handling:**
- Per architecture decisions: continue on per-repo failure (worktree dir might be missing)
- Print warning for failed repos, don't abort

**Tests:**
- Unit test with TestEnv: create repos, write meta, verify status output
- Edge: worktree dir missing for one repo → warning, continues to next
- Test auto-detect (no name arg, cwd inside forest)

---

## Command: `exec <name> -- <cmd>`

**What it does:** Run an arbitrary command in each repo directory of a forest.

**Flow:**
1. Load config → `worktree_base`
2. `find_forest(worktree_base, name)` → `(forest_dir, meta)`
3. For each repo in `meta.repos`:
   - Compute worktree path: `forest_dir / repo.name`
   - Print header: `=== repo.name ===`
   - Run command via `std::process::Command` with inherited stdout/stderr
   - Track exit codes
4. If any command failed, exit with non-zero

**Important:** This is NOT `git()` — it runs arbitrary commands. Need a new helper or use `std::process::Command` directly. Could add a `run_stream` helper in a new util or extend what's in `git.rs` to be more general.

**Error handling:**
- Per architecture decisions: continue on failure, report non-zero exits at end
- If command not found, report and continue

**Tests:**
- Unit test: exec `echo hello` across repos, verify it runs in each dir
- Edge: command fails in one repo → continues, reports failure
- Edge: empty cmd vec → error

---

## Suggested implementation order

1. **Shared plumbing** — `default_config_path`, `resolve_forest`, `commands.rs` module structure
2. **`ls`** — simplest command, exercises config loading + forest discovery
3. **`status`** — exercises forest resolution + git wrapper
4. **`exec`** — exercises forest resolution + arbitrary command execution

Each step builds on the previous. Steps 2-4 could each be a commit.

## What changes in which files

| File | Changes |
|------|---------|
| `config.rs` | Add `default_config_path()` |
| `forest.rs` | Add `resolve_forest()` |
| `commands.rs` | **New file** — `cmd_ls`, `cmd_status`, `cmd_exec` |
| `main.rs` | Replace stubs with config load → handler calls |
| `cli.rs` | No changes expected |
| `testutil.rs` | May need helpers to create fake worktree dirs within forests |

## Open questions (resolve during implementation)

- **`exec` auto-detect:** The current CLI requires `name` for exec (not optional). Should we make it optional with auto-detect like status? Probably yes for consistency, but it's a minor CLI change we can do in Phase 5.
- **Output formatting:** Should `ls` use fixed-width columns or something simpler? Start simple, polish later.
- **Config missing vs. worktree_base missing:** These are different errors. Config missing → "run `git forest init`". Config exists but worktree_base doesn't → that's fine, just means no forests yet.
