# Phase 2 Plan — `init` (Config Generation)

**STATUS: DRAFT**

## Goal

Implement `git forest init` to generate `~/.config/git-forest/config.toml`. Two layers: a non-interactive flag-based interface (agent-friendly, testable) and an interactive wizard for humans. Both share the same validation and write path.

## Design Principle: Agent-Drivable First

Every command should be drivable by a software agent (MCP tool, shell script, AI coding assistant) without simulating TTY input. This means:

1. **All inputs expressible as flags.** The interactive wizard is a convenience layer, not the only path.
2. **Structured, predictable output.** Errors to stderr with actionable next steps. Success to stdout. Exit codes are meaningful (0 = success, 1 = user error, 2 = system error).
3. **Idempotent when possible.** `init` with `--force` and the same flags = same result.
4. **No hidden state.** The config file is the only artifact. Its path is discoverable via `git forest init --show-path`.

This doesn't mean we build `--json` output or MCP integration now — it means we design the internal architecture so these are easy to add later. Specifically: command logic is a pure function from inputs to outputs, with IO at the edges.

## Architecture

```
CLI flags / interactive prompts
        │
        ▼
  InitInputs (struct)
        │
        ▼
  validate_init_inputs() -> Result<ResolvedConfig>
        │
        ▼
  write_config_atomic(path, config) -> Result<()>
```

`InitInputs` is the boundary. Everything above it is UI (flags or dialoguer). Everything below it is pure logic + IO. This means:
- Tests exercise `validate_init_inputs()` directly — no prompt mocking needed.
- A future MCP tool calls the same validation/write path.
- The interactive wizard is just one way to populate `InitInputs`.

## Phase 2a — Non-Interactive Core

### CLI changes

```
git forest init [flags]
    --worktree-base <PATH>     Where to create forests (default: ~/worktrees)
    --base-branch <BRANCH>     Default base branch (default: dev)
    --branch-template <TPL>    Branch naming template (default: {user}/{name})
    --username <NAME>          Your short username
    --repo <PATH>              Add a repo (repeatable)
    --force                    Overwrite existing config without prompting
    --show-path                Print config path and exit
```

Minimal valid invocation:
```sh
git forest init \
  --username dliv \
  --repo ~/src/foo-api \
  --repo ~/src/foo-web
```

Everything else has sensible defaults.

### `InitInputs` struct

```rust
pub struct InitInputs {
    pub worktree_base: String,      // raw, pre-expansion (e.g. "~/worktrees")
    pub base_branch: String,
    pub branch_template: String,
    pub username: String,
    pub repos: Vec<RepoInput>,
}

pub struct RepoInput {
    pub path: String,               // raw, pre-expansion
    pub name: Option<String>,       // override; derived from path if None
    pub base_branch: Option<String>, // override; inherited from general if None
}
```

### Validation (`validate_init_inputs`)

- Expand tildes in all paths
- Validate `worktree_base` parent exists (we'll create `worktree_base` itself)
- Validate each repo `path` exists and is a git repo (`git rev-parse --git-dir`)
- Derive repo names from paths when not explicit
- Check for duplicate repo names
- Validate `branch_template` contains `{name}`
- Return `ResolvedConfig` on success, rich error on failure

### Atomic write

- Write to `<config_path>.tmp`
- Rename to `<config_path>`
- Create parent directories if needed
- If config already exists and `--force` not passed: error with message "Config already exists at <path>. Use --force to overwrite, or edit it directly."

### Error messages (agent-friendly)

Every error should include:
- What went wrong
- What the user (or agent) should do about it

Examples:
```
error: ~/src/foo-api is not a git repository
  hint: run `git init` in that directory, or check the path

error: config already exists at ~/.config/git-forest/config.toml
  hint: use `--force` to overwrite, or edit the file directly

error: repo name 'foo' is used by both ~/src/foo and ~/src/other/foo
  hint: use `--repo ~/src/other/foo:name=other-foo` to disambiguate
  (or in interactive mode, you'll be prompted for a name)
```

### What changes in which files

| File | Changes |
|------|---------|
| `cli.rs` | Add flags to `Init` variant |
| `config.rs` | Add `write_config_atomic()`, make `Config` serializable (already is) |
| `commands.rs` | Add `cmd_init()` — parse flags into `InitInputs`, validate, write |
| `main.rs` | Wire up `Init` to `cmd_init()` |
| `Cargo.toml` | No new deps yet (dialoguer comes in 2b) |

### Tests

**Unit tests (in `commands.rs` or a new `init.rs`):**
- Valid inputs → produces correct `ResolvedConfig`
- Missing username → error
- No repos → error (at least one repo required)
- Duplicate repo names → error with hint
- Invalid repo path (not a git repo) → error with hint
- `branch_template` missing `{name}` → error
- Tilde expansion works in all path fields

**Integration tests (in `tests/cli_test.rs`):**
- `init --show-path` prints path and exits 0
- `init --username dliv --repo <path>` creates config file
- `init` without `--username` or `--repo` → error (in non-interactive mode; once 2b lands, this triggers the wizard instead)
- `init` when config exists → error mentioning `--force`
- `init --force` when config exists → overwrites
- Update existing test `subcommand_init_recognized` (currently expects "not yet implemented")

---

## Phase 2b — Interactive Wizard

Add `dialoguer` dependency. When `init` is run without sufficient flags, fall back to interactive prompts.

### Detection logic

```
if stdin is a TTY and required flags are missing:
    launch wizard (pre-fill any flags that were provided)
else if required flags are missing:
    error with usage help
else:
    proceed non-interactively
```

This means an agent piping input or passing flags never hits the wizard.

### Wizard flow

1. **Username** — `Input::new("Your short username (for branch names)")` with default from `$USER` or git config `user.name`
2. **Worktree base** — `Input::new("Where should forests live?")` with default `~/worktrees`
3. **Base branch** — `Input::new("Default base branch")` with default `dev`
4. **Branch template** — `Input::new("Branch naming template")` with default `{user}/{name}`, validate contains `{name}`
5. **Repos** — Loop:
   - `Input::new("Path to a repo (empty to finish)")`
   - Auto-derive name, show it: `"  → name: foo-api"`
   - Optional: `"Override base branch for foo-api? (default: dev)"` — skip if user just hits enter
   - Repeat until empty input
   - Require at least one repo
6. **Confirm** — Show summary, `Confirm::new("Write config?")`

### What changes

| File | Changes |
|------|---------|
| `Cargo.toml` | Add `dialoguer` dependency |
| `commands.rs` | Add wizard function that returns `InitInputs` |

### Tests

Interactive prompts are hard to unit test. Strategy:
- The wizard function returns `InitInputs` — the same struct flags produce
- All validation is in `validate_init_inputs()` — already tested in 2a
- Integration test: `init` with flags still works (no wizard triggered)
- Manual testing for the wizard UX

---

## Open Questions (Resolve During Implementation)

1. **Repo path syntax for name/base_branch overrides in flags.** Proposed: `--repo ~/src/foo:name=custom-name,base_branch=main`. Alternatively, separate flags like `--repo-override foo:base_branch=main`. The colon syntax is more compact but harder to parse. Could also defer overrides to config file editing and keep `--repo` simple (just paths).

2. **Should `init` validate that repos have the expected base branch?** e.g., if user says `base_branch = dev`, check that `origin/dev` exists. Probably yes as a warning, not an error — the remote might not be set up yet.

3. **Re-running init: merge or replace?** Architecture doc says "Overwrite with confirmation, or merge with existing config?" Recommendation: replace with `--force`. Merging is complex and error-prone. If you want to add a repo, edit the config file. We could add `git forest config add-repo <path>` later.

## Out of Scope

- `--json` output (Phase 5)
- MCP tool integration (post-v1)
- Config migrations / schema versioning (post-v1)
- `git forest config` subcommand for editing (post-v1)
