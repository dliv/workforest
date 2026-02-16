# Phase 0 — Crate Skeleton, Foundation, and Test Harness

## Goal

A runnable `git-forest` CLI with shared infrastructure and a real integration test harness. `git forest --help` works. All foundation types are tested against real git repos. No git mutations beyond what tests need. No interactive prompts.

---

## What to Implement

### 1. Crate Setup

- `cargo init --name git-forest`
- Dependencies: `clap` (derive), `serde` + `toml`, `chrono`, `directories`, `anyhow` (or `thiserror`)
- Dev dependencies: `tempfile`, `assert_cmd` (for CLI integration tests)
- Binary installs as `git-forest` (invoked as `git forest`)
- Clap subcommands stubbed: `init`, `new`, `rm`, `ls`, `status`, `exec`
  - Each prints "not yet implemented" and exits cleanly

### 2. Config Types + Loading

```rust
struct Config {
    general: GeneralConfig,
    repos: Vec<RepoConfig>,
}

struct GeneralConfig {
    worktree_base: PathBuf,       // expanded from ~
    base_branch: String,          // e.g., "dev"
    branch_template: String,      // e.g., "{user}/{name}"
    username: String,
}

struct RepoConfig {
    path: PathBuf,                // expanded from ~
    name: String,                 // derived from path if not specified
    base_branch: String,          // inherited from general if not specified
    remote: String,               // defaults to "origin"
}
```

- `load_config(path: &Path) -> Result<Config>`
- Tilde expansion on `worktree_base` and repo `path` fields
- Derive `name` from last segment of `path` if not specified
- Inherit `base_branch` from `general` if not specified per repo
- Default `remote` to `"origin"` if not specified
- Validation:
  - All repo names are unique
  - All repo paths exist and are git repos (defer this check? or make it a warning?)
  - `worktree_base` parent directory exists
  - `branch_template` contains `{name}` (at minimum)

### 3. Forest Meta Types + Read/Write

```rust
struct ForestMeta {
    name: String,
    created_at: DateTime<Utc>,
    mode: ForestMode,             // "feature" or "review"
    repos: Vec<RepoMeta>,
}

enum ForestMode {
    Feature,
    Review,
}

struct RepoMeta {
    name: String,
    source: PathBuf,              // absolute, expanded
    branch: String,
    base_branch: String,
    branch_created: bool,
}
```

- `ForestMeta::write(path: &Path) -> Result<()>`
- `ForestMeta::read(path: &Path) -> Result<ForestMeta>`
- Write is incremental-friendly: header first, then repos can be appended
  - For Phase 0, implement simple full-file write. Incremental write is a Phase 3 concern.

### 4. Git Wrapper

```rust
fn git(repo: &Path, args: &[&str]) -> Result<String>
fn git_stream(repo: &Path, args: &[&str]) -> Result<ExitStatus>
```

- `git()`: captures stdout, returns as String. On non-zero exit, returns error with command, args, cwd, exit code, stderr.
- `git_stream()`: inherits stdout/stderr (for user-facing output). Returns ExitStatus.

### 5. Path Helpers

- `expand_tilde(path: &str) -> PathBuf` — replaces leading `~` with `$HOME`
- `sanitize_forest_name(name: &str) -> String` — replaces `/` with `-`, validates no weird chars
- `forest_dir(worktree_base: &Path, name: &str) -> PathBuf` — `{worktree_base}/{sanitize(name)}`

### 6. Forest Discovery

- `discover_forests(worktree_base: &Path) -> Result<Vec<ForestMeta>>` — scans direct children of `worktree_base` for `.forest-meta.toml`
- `find_forest(worktree_base: &Path, name_or_dir: &str) -> Result<Option<ForestMeta>>` — matches against meta `name` or directory name
- `detect_current_forest() -> Result<Option<ForestMeta>>` — walks up from cwd looking for `.forest-meta.toml`

---

## Test Harness (`TestEnv`)

Lives in `tests/common/mod.rs` (or `tests/helpers/mod.rs`).

```rust
struct TestEnv {
    dir: TempDir,                 // root temp directory
    // Layout:
    //   {dir}/src/               — source repos live here
    //   {dir}/worktrees/         — forest worktrees go here
    //   {dir}/config/            — config.toml goes here
}

impl TestEnv {
    fn new() -> Self;

    // Create a real git repo with an initial commit
    fn create_repo(&self, name: &str) -> PathBuf;

    // Create a repo with a specific branch
    fn create_repo_with_branch(&self, name: &str, branch: &str) -> PathBuf;

    // Write a config.toml with the given repos
    fn write_config(&self, config: &Config);

    // Path helpers
    fn config_path(&self) -> PathBuf;
    fn src_dir(&self) -> PathBuf;
    fn worktree_base(&self) -> PathBuf;
    fn repo_path(&self, name: &str) -> PathBuf;

    // Build a Config for this env's repos
    fn default_config(&self, repo_names: &[&str]) -> Config;
}
```

Each `create_repo` call does:
1. `git init {dir}/src/{name}`
2. `git commit --allow-empty -m "initial"` (so branches can be created)
3. Returns the path

This is the foundation that Phase 1–4 tests build on. Phase 3 tests will add methods like `env.assert_worktree_exists("forest-name", "foo-api")`.

---

## What to Test

### Config loading
- Parse a valid config with all fields
- Parse minimal config (only required fields, defaults applied)
- Tilde expansion works on `worktree_base` and repo `path`
- `name` derived from path when omitted
- `base_branch` inherited from general when omitted per repo
- `remote` defaults to `"origin"`
- Validation: duplicate repo names error
- Validation: missing required fields error

### Meta read/write
- Round-trip: write then read, values match
- Read a meta file with all fields
- Handles both `feature` and `review` modes

### Git wrapper
- `git()` captures output from a real repo (e.g., `git rev-parse HEAD`)
- `git()` returns error on bad command with stderr in error
- `git_stream()` runs and returns exit status

### Path helpers
- `expand_tilde` replaces `~` with home dir
- `expand_tilde` leaves absolute paths unchanged
- `sanitize_forest_name` replaces `/` with `-`
- `sanitize_forest_name` handles edge cases (leading dots, empty, etc.)

### Forest discovery
- Discovers forests from meta files in worktree_base children
- Returns empty vec when no forests exist
- `find_forest` matches by meta name
- `find_forest` matches by directory name
- `detect_current_forest` finds meta when cwd is inside a forest

### CLI smoke tests (with `assert_cmd`)
- `git-forest --help` exits 0
- `git-forest ls` runs without config (or with helpful error)
- Each subcommand is recognized

---

## What to Defer

- Interactive prompts (Phase 2–3)
- Actual worktree creation/deletion (Phase 3–4)
- The `init` wizard (Phase 2)
- `new`, `rm` implementation (Phase 3–4)
- Incremental meta writing (Phase 3 — use full-file write for now)
- Branch resolution logic (Phase 3)
- Pretty output formatting (Phase 5)

---

## File Layout

Tests are co-located in each source file using `#[cfg(test)] mod tests`. The shared `TestEnv` harness lives in `src/testutil.rs` (behind `#[cfg(test)]`) so it's accessible from all modules. Only CLI smoke tests (which test the binary externally via `assert_cmd`) live in `tests/`.

```
src/
  main.rs           — clap setup + subcommand dispatch
  cli.rs            — clap derive structs
  config.rs         — Config types + load_config() + #[cfg(test)] mod tests
  meta.rs           — ForestMeta types + read/write + #[cfg(test)] mod tests
  git.rs            — git() and git_stream() wrappers + #[cfg(test)] mod tests
  paths.rs          — expand_tilde, sanitize_forest_name, forest_dir + #[cfg(test)] mod tests
  forest.rs         — discover_forests, find_forest, detect_current_forest + #[cfg(test)] mod tests
  testutil.rs       — #[cfg(test)] TestEnv harness (shared across all module tests)
tests/
  cli_test.rs       — CLI smoke tests via assert_cmd (binary-level)
```
