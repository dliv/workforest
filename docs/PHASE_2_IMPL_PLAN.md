# Phase 2 Implementation Plan

## Context

Phase 0 (foundation) and Phase 1 (read-only commands) are complete with 58 passing tests. The current commands (`ls`, `status`, `exec`) work but print directly via `println!`. Per architecture decisions 7-9, we need to:

1. Refactor commands to return typed data structs (not print)
2. Add `--json` global flag for machine-readable output
3. Implement `init` as a flag-driven, non-interactive command
4. Add `debug_assert!` postconditions per Decision 10

This enables agent-driven workflows and makes tests assert on data instead of "doesn't panic."

## Commits

Six commits, matching the Phase 2 plan's suggested implementation order. Each is independently buildable and testable.

### Commit 1: Add `serde_json` dep + `--json` global flag

**Files:**
- `Cargo.toml` — add `serde_json = "1"` to `[dependencies]`
- `src/cli.rs` — add `#[arg(long, global = true)] pub json: bool` to `Cli`

No logic changes. Just wiring. Existing tests pass unchanged. `--help` will show the new flag.

### Commit 2: Refactor `cmd_ls` to return data

**File: `src/commands.rs`**

Add result structs with doc comments explaining the pattern (Decision 8):

```rust
/// Result structs for command output. Commands return these instead of printing
/// directly — main.rs formats them as human-readable or JSON based on --json.
/// See architecture-decisions.md, Decision 8.

#[derive(Debug, Serialize)]
pub struct LsResult {
    pub forests: Vec<ForestSummary>,
}

#[derive(Debug, Serialize)]
pub struct ForestSummary {
    pub name: String,
    pub age_seconds: i64,
    pub age_display: String,
    pub mode: ForestMode,
    pub branch_summary: Vec<BranchCount>,
}

#[derive(Debug, Serialize)]
pub struct BranchCount {
    pub branch: String,
    pub count: usize,
}
```

Change `cmd_ls(worktree_base) -> Result<()>` to `cmd_ls(worktree_base) -> Result<LsResult>`.

Move `format_age` to take `i64` seconds (pure, no `ForestMeta` dependency). Keep `format_branches` as a helper but have it work on `&[BranchCount]`.

Add `pub fn format_ls_human(result: &LsResult) -> String` for the table output.

**File: `src/main.rs`**

Update `Ls` arm: call `cmd_ls`, then format based on `cli.json`:
```rust
Command::Ls => {
    let config = config::load_default_config()?;
    let result = commands::cmd_ls(&config.general.worktree_base)?;
    output(&result, cli.json, commands::format_ls_human)?;
}
```

Add a small `output` helper in main.rs:
```rust
fn output<T: serde::Serialize>(result: &T, json: bool, human_fn: fn(&T) -> String) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
    } else {
        let text = human_fn(result);
        if !text.is_empty() {
            println!("{}", text);
        }
    }
    Ok(())
}
```

**File: `src/meta.rs`**

`ForestMode` already has `Serialize` — confirmed it works with serde_json.

**Tests in `src/commands.rs`:**
- `cmd_ls_empty_worktree_base` → assert `result.forests.is_empty()`
- `cmd_ls_nonexistent_dir` → assert `result.forests.is_empty()`
- `cmd_ls_with_forests` → assert `result.forests.len() == 2`, check names, modes, branch_summary
- `format_age_*` tests stay the same (refactored to take `i64`)
- `format_branches_*` → refactored to work on `&[BranchCount]`
- New: `format_ls_human` test with known data → check table string

### Commit 3: Refactor `cmd_status` to return data

**File: `src/commands.rs`**

```rust
#[derive(Debug, Serialize)]
pub struct StatusResult {
    pub forest_name: String,
    pub repos: Vec<RepoStatus>,
}

#[derive(Debug, Serialize)]
pub struct RepoStatus {
    pub name: String,
    pub status: RepoStatusKind,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum RepoStatusKind {
    Ok { output: String },
    Missing { path: String },
    Error { message: String },
}
```

Change `cmd_status` to return `Result<StatusResult>`. Add `format_status_human`.

**File: `src/main.rs`** — update `Status` arm.

**Tests:**
- `cmd_status_runs_in_each_repo` → assert `result.repos.len() == 2`, each is `RepoStatusKind::Ok`
- `cmd_status_missing_worktree_continues` → assert the repo has `RepoStatusKind::Missing`

### Commit 4: Refactor `cmd_exec` to return data

`exec` is special — it streams stdout/stderr in real time. We keep the streaming but return a summary.

```rust
#[derive(Debug, Serialize)]
pub struct ExecResult {
    pub forest_name: String,
    pub failures: Vec<String>,
}
```

Change `cmd_exec` to return `Result<ExecResult>` instead of calling `process::exit(1)`. Move the exit-on-failure logic to `main.rs`.

`format_exec_human` — just formats the failure summary (the streamed output already went to the terminal).

For `--json` mode with exec: the streamed output still goes to stderr/stdout inherited. The JSON result at the end captures the summary only. This is a pragmatic compromise — full JSON capture of exec output would require buffering, which breaks the streaming UX.

**Tests:**
- `cmd_exec_runs_command_in_each_repo` → assert `result.failures.is_empty()`
- `cmd_exec_empty_cmd_errors` → stays as `assert!(result.is_err())`
- Remove the `process::exit(1)` from cmd_exec (move to main.rs)

### Commit 5: Implement `cmd_init`

**File: `src/cli.rs`**

Expand `Init` variant with flags:
```rust
Init {
    #[arg(long, default_value = "~/worktrees")]
    worktree_base: String,
    #[arg(long, default_value = "dev")]
    base_branch: String,
    #[arg(long, default_value = "{user}/{name}")]
    branch_template: String,
    #[arg(long)]
    username: Option<String>,
    #[arg(long = "repo")]
    repos: Vec<String>,
    #[arg(long)]
    force: bool,
    #[arg(long)]
    show_path: bool,
}
```

`username` and `repos` are required at validation time (not clap-level) so we can give better error messages with hints.

**File: `src/commands.rs`** (or new `src/init.rs` if it gets big — start in commands.rs)

```rust
pub struct InitInputs {
    pub worktree_base: String,
    pub base_branch: String,
    pub branch_template: String,
    pub username: String,
    pub repos: Vec<RepoInput>,
}

pub struct RepoInput {
    pub path: String,
    pub name: Option<String>,
    pub base_branch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InitResult {
    pub config_path: PathBuf,
    pub worktree_base: PathBuf,
    pub repos: Vec<InitRepoSummary>,
}

#[derive(Debug, Serialize)]
pub struct InitRepoSummary {
    pub name: String,
    pub path: PathBuf,
    pub base_branch: String,
}
```

`validate_init_inputs(inputs: &InitInputs) -> Result<ResolvedConfig>`:
- Expand tildes in all paths
- Validate worktree_base parent exists
- Validate each repo path exists and is a git repo (`git rev-parse --git-dir`)
- Derive names, check duplicates
- Validate branch_template contains `{name}`
- `debug_assert!` postconditions: all paths absolute, names non-empty

`cmd_init(inputs: InitInputs, config_path: &Path, force: bool) -> Result<InitResult>`:
- Call validate_init_inputs
- Call write_config_atomic
- Return InitResult

**File: `src/config.rs`**

Add `write_config_atomic(path: &Path, config: &ResolvedConfig, force: bool) -> Result<()>`:
- Check if file exists and !force → error with hint
- Create parent dirs
- Build `Config` from `ResolvedConfig` (reverse of parse)
- Serialize to TOML
- Write to `path.with_extension("toml.tmp")`
- Rename to `path`

`format_init_human` — print config path and repo summary.

**File: `src/main.rs`** — wire up Init with the new flags.

**Tests (unit, in commands.rs):**
- Valid inputs → correct ResolvedConfig + InitResult
- Missing username → error with hint
- Empty repos → error with hint
- Duplicate repo names → error with hint
- Repo path that isn't a git repo → error with hint
- branch_template missing `{name}` → error
- Tilde expansion in worktree_base and repo paths
- write_config_atomic creates file
- write_config_atomic with force overwrites
- write_config_atomic without force on existing file → error

### Commit 6: Integration tests + debug_assert! pass

**File: `tests/cli_test.rs`**

- Update `subcommand_init_recognized`: change from "not yet implemented" to checking for a meaningful error (missing --username)
- `init_show_path` → exits 0, stdout contains config path
- `init_creates_config` → create temp git repo, run init with flags, verify config file exists and is valid TOML
- `init_force_overwrites` → run init twice, second with --force
- `init_without_force_errors` → run init twice without --force, second fails
- `ls_json_flag` → run ls --json with mocked config (hard to do without real config; may skip or create one in tmp)

**File: various src/ files** — sprinkle `debug_assert!` at key boundaries:
- `parse_config`: postcondition — all paths absolute, names non-empty, no duplicates
- `expand_tilde`: postcondition — result doesn't start with `~/`
- `sanitize_forest_name`: postcondition — result doesn't contain `/`
- `validate_init_inputs`: postcondition — all paths absolute

## Verification

After each commit:
```
just check   # fmt + clippy
just test    # all tests pass
```

After all commits:
```
just test    # 58 existing + ~15-20 new tests all pass
cargo run -- --help  # shows --json flag
cargo run -- init --show-path  # prints config path
cargo run -- init --username test --repo /tmp/some-git-repo  # creates config
cargo run -- init --json --username test --repo /tmp/some-git-repo --force  # JSON output
```
