# Phase 2 Plan — `init` + Output Architecture

**STATUS: COMPLETE** — All items implemented. 75 tests (63 unit + 12 integration).

## Goal

Two things in this phase:

1. Implement `git forest init` as a non-interactive, flag-driven command.
2. Refactor all commands to return data (not print), and add `--json` output support.

The interactive wizard (dialoguer) is deferred to Phase 5 — the flag-based interface is sufficient for development and is the primary interface for agents.

## Design Principles

This phase implements Decisions 7–9 from [architecture-decisions.md](architecture-decisions.md): agent-drivable first, commands return data, and plan/execute for mutating commands. See that document for the full rationale.

## Architecture

### Output flow (all commands)

```
main.rs: parse CLI, load config, resolve args
        │
        ▼
  cmd_xxx(inputs) -> Result<XxxResult>    ← pure-ish, no printing
        │
        ▼
  main.rs: match --json
        ├── true:  serde_json::to_string_pretty(&result)
        └── false: format_xxx_human(&result) -> String
        │
        ▼
  println!
```

### Init flow (plan/execute)

```
CLI flags
    │
    ▼
InitInputs (struct)
    │
    ▼
validate_init_inputs() -> Result<ResolvedConfig>    ← pure: plan
    │
    ▼
write_config_atomic(path, config) -> Result<()>     ← impure: execute
    │
    ▼
InitResult { path, config_summary }
```

---

## Step 1 — Refactor Existing Commands to Return Data

Before adding `init`, refactor `ls`, `status`, and `exec` so they return data instead of printing. This establishes the pattern for all future commands.

### Result structs

```rust
// ls
#[derive(Serialize)]
pub struct LsResult {
    pub forests: Vec<ForestSummary>,
}

#[derive(Serialize)]
pub struct ForestSummary {
    pub name: String,
    pub age_seconds: i64,       // raw; human formatter turns this into "3d ago"
    pub age_display: String,    // pre-formatted for human output
    pub mode: ForestMode,
    pub branch_summary: Vec<BranchCount>,
}

#[derive(Serialize)]
pub struct BranchCount {
    pub branch: String,
    pub count: usize,
}

// status
#[derive(Serialize)]
pub struct StatusResult {
    pub forest_name: String,
    pub repos: Vec<RepoStatus>,
}

#[derive(Serialize)]
pub struct RepoStatus {
    pub name: String,
    pub status: RepoStatusKind,
}

#[derive(Serialize)]
pub enum RepoStatusKind {
    Ok { output: String },
    Missing { path: String },
    Error { message: String },
}
```

`exec` is special — it streams output and must pass through stdout/stderr in real time. It stays as `Result<ExecResult>` where `ExecResult` just captures the summary (which repos failed). The streaming happens during execution; the result is reported at the end.

### Global `--json` flag

Add to `Cli`:
```rust
#[derive(Parser)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}
```

### What changes

| File | Changes |
|------|---------|
| `cli.rs` | Add `--json` global flag |
| `commands.rs` | Return result structs instead of printing. Extract human formatting into `format_*` functions. |
| `main.rs` | Add output dispatch: `--json` → JSON, else → human format |
| `Cargo.toml` | Add `serde_json` dependency |

### Tests

- Existing unit tests switch from "doesn't panic" to asserting on returned data (much better).
- Add tests for `format_ls_human()` etc. if the formatting logic is non-trivial.
- Integration test: `ls --json` returns valid JSON.

---

## Step 2 — Implement `init`

### CLI

```
git forest init [flags]
    --worktree-base <PATH>     Where to create forests (default: ~/worktrees)
    --base-branch <BRANCH>     Default base branch (default: dev)
    --branch-template <TPL>    Branch naming template (default: {user}/{name})
    --username <NAME>          Your short username (required)
    --repo <PATH>              Add a repo (repeatable, at least one required)
    --force                    Overwrite existing config without prompting
    --show-path                Print config path and exit
```

Minimal invocation:
```sh
git forest init \
  --username dliv \
  --repo ~/src/foo-api \
  --repo ~/src/foo-web
```

Without required flags: error with usage help (no wizard fallback — that's Phase 5).

### `InitInputs` struct

```rust
pub struct InitInputs {
    pub worktree_base: String,       // raw, pre-expansion (e.g. "~/worktrees")
    pub base_branch: String,
    pub branch_template: String,
    pub username: String,
    pub repos: Vec<RepoInput>,
}

pub struct RepoInput {
    pub path: String,                // raw, pre-expansion
    pub name: Option<String>,        // override; derived from path if None
    pub base_branch: Option<String>, // override; inherited from general if None
}
```

### Validation (`validate_init_inputs`)

- Expand tildes in all paths
- Validate `worktree_base` parent directory exists (we create `worktree_base` itself if needed)
- Validate each repo `path` exists and is a git repo (`git rev-parse --git-dir`)
- Derive repo names from paths when not explicit
- Check for duplicate repo names
- Validate `branch_template` contains `{name}`
- Return `ResolvedConfig` on success, rich error on failure

### Atomic write

- Create parent directories if needed
- Write to `<config_path>.tmp`
- Rename to `<config_path>`
- If config exists and `--force` not set: error with hint

### Result struct

```rust
#[derive(Serialize)]
pub struct InitResult {
    pub config_path: PathBuf,
    pub worktree_base: PathBuf,
    pub repos: Vec<InitRepoSummary>,
}

#[derive(Serialize)]
pub struct InitRepoSummary {
    pub name: String,
    pub path: PathBuf,
    pub base_branch: String,
}
```

### Error messages

Every error includes what went wrong and what to do about it:

```
error: ~/src/foo-api is not a git repository
  hint: run `git init` in that directory, or check the path

error: config already exists at ~/.config/git-forest/config.toml
  hint: use --force to overwrite, or edit the file directly

error: repo name 'foo' is used by both ~/src/foo and ~/src/other/foo
  hint: add an explicit name with a separate --repo-name flag,
        or rename one of the directories
```

### What changes

| File | Changes |
|------|---------|
| `cli.rs` | Add flags to `Init` variant |
| `config.rs` | Add `write_config_atomic()` |
| `commands.rs` | Add `cmd_init()` returning `InitResult`, `validate_init_inputs()` |
| `main.rs` | Wire up `Init`, format output |

### Tests

**Unit tests:**
- Valid inputs → correct `ResolvedConfig` + `InitResult`
- Missing username → error
- No repos → error
- Duplicate repo names → error with hint
- Invalid repo path (not a git repo) → error with hint
- `branch_template` missing `{name}` → error
- Tilde expansion in all path fields
- Atomic write creates file
- Atomic write with `--force` overwrites
- Atomic write without `--force` when file exists → error

**Integration tests:**
- `init --show-path` prints path and exits 0
- `init --username dliv --repo <tmpdir>/repo` creates valid config
- `init` without required flags → error
- `init` when config exists → error mentioning `--force`
- `init --force` → overwrites
- `init --json --username dliv --repo <path>` → valid JSON result
- Update existing `subcommand_init_recognized` test

---

## Suggested Implementation Order

1. **Add `serde_json` dep + `--json` flag to CLI** — mechanical, no logic changes yet
2. **Refactor `cmd_ls`** — return `LsResult`, move printing to `main.rs`. Update tests.
3. **Refactor `cmd_status`** — return `StatusResult`, same pattern. Update tests.
4. **Refactor `cmd_exec`** — lighter touch (streaming stays), just capture `ExecResult` summary.
5. **Implement `cmd_init`** — `InitInputs` → validate → write → `InitResult`.
6. **Integration tests** for init + --json across commands.

Each step is a clean commit.

---

## Open Questions (Resolve During Implementation)

1. **Repo overrides via flags.** For now, `--repo` is just a path. Per-repo `base_branch` overrides require editing the config file after init. We can add `--repo-base-branch <name>=<branch>` or similar later if needed, but keeping `--repo` simple is better for now.

2. **Should `init` warn if `origin/<base_branch>` doesn't exist?** Probably yes as a warning (not error) — the remote might not be set up yet. Defer to implementation.

3. **Re-running init: replace, not merge.** `--force` overwrites entirely. Merging is complex and error-prone. For adding repos later, edit the file or (eventually) `git forest config add-repo`.

## Out of Scope (Deferred)

- Interactive wizard with `dialoguer` → Phase 5
- MCP tool integration → post-v1
- Config migrations / schema versioning → post-v1
- `git forest config` subcommand → post-v1
- `--verbose` flag → Phase 5
