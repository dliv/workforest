# git-forest — Architecture Decisions & Project Plan

This document captures architectural decisions, open questions, and the phased build plan for `git-forest`. The original spec lives in [claude-web-init.md](claude-web-init.md) but has been **superseded** by this document on repo types (consolidated to one), config schema, and forest meta format.

---

## Decisions to Lock Down Before Coding

These are shared contracts that multiple commands depend on. Changing them later causes rework.

### 1. Config Schema

The config lives at `~/.config/git-forest/config.toml` (XDG).

**DECIDED: field naming — use `base_branch` everywhere.** The original spec mixed `base_branch` (general) and `branch_base` (repo override). Unified on `base_branch` at both levels.

**DECIDED: one repo type.** The original spec had three types (`mutable`, `branch-on-main`, `readonly`). These have been consolidated into a single repo concept. Every repo is a worktree. The only per-repo config that matters is `base_branch` (if it differs from the global default). Whether you modify a repo in a given forest is your choice at the time, not something encoded in config.

- Repos that branch off `dev` (company repos): inherit the global `base_branch`.
- Repos that branch off `main` (personal repos, coworker references): set `base_branch = "main"`.
- In review mode, repos that aren't the focus get a `forest/{forest-name}` branch off their base. In feature mode, they get the suggested feature branch.

**DECIDED: repo `name` is derived from `path` by default.** `name` is optional in config. If omitted, it defaults to the last path segment (e.g., `~/src/foo-api` → `foo-api`). Explicit `name` is allowed as an override but should be rare — it changes the folder name inside forests, which can break relative paths. The `init` wizard should warn if the user overrides.

**DECIDED: per-repo `remote` field — optional, defaults to `"origin"`.** Multi-remote discovery is out of scope for v1.

Example config:

```toml
[general]
worktree_base = "~/worktrees"
base_branch = "dev"
branch_template = "{user}/{name}"
username = "dliv"

[[repos]]
path = "~/src/foo-api"        # name "foo-api" implicit, base_branch "dev" inherited

[[repos]]
path = "~/src/foo-web"        # name "foo-web" implicit, base_branch "dev" inherited

[[repos]]
path = "~/src/foo-infra"      # name "foo-infra" implicit, base_branch "dev" inherited

[[repos]]
path = "~/src/dev-docs"       # personal agentic context
base_branch = "main"

[[repos]]
path = "~/src/bar-workspace"  # coworker's repo, referenced but rarely modified
base_branch = "main"
```

### 2. Path Handling

**DECIDED: tilde expansion.** Expand `~` to `$HOME` when loading config. No arbitrary env var expansion. Meta files (`.forest-meta.toml`) store fully expanded absolute paths so they are self-contained and never require re-expansion.

**DECIDED: sanitization — detect collisions and error.** Forest names may contain `/` (e.g., `java-84/refactor-auth`), which maps to `java-84-refactor-auth` on disk. If that directory already exists (from a different forest name like `java-84-refactor-auth`), error with a helpful message suggesting the user pick a different name.

### 3. Forest Identity & Lookup

**DECIDED: accept both original and sanitized names.** Resolve by scanning `.forest-meta.toml` files in direct children of `worktree_base` (not recursive). Match against both the meta `name` field and the directory name.

**DECIDED: auto-detect current forest.** If the user is inside a forest directory and omits `<name>`, detect the current forest by walking up to `.forest-meta.toml`. Applies to `status`, `exec`, and `rm`. Not `new`.

### 4. Git Wrapper & Error Model

**DECIDED.** All git operations go through a helper function. Two modes:

- **Capture:** `git(repo, args) -> Result<String>` — for commands where we need the output (branch checks, status).
- **Stream:** `git_stream(repo, args) -> Result<ExitStatus>` — for commands where output should pass through to the user (exec, fetch).

**Error type** should include: command, args, working directory, exit code, stderr.

**Continue-on-error policy:**
- `exec`: continue to next repo on failure, report non-zero exit at the end.
- `new`: stop on failure, leave partial forest (meta already written incrementally).
- `rm`: best-effort cleanup, continue on individual failures, report all errors at the end.
- `status`: continue on failure (a repo dir might be missing).

### 5. Partial Failure in `new`

**DECIDED: write `.forest-meta.toml` incrementally.** Start with the forest header, append each repo entry as it's successfully created. This way `rm` can always clean up whatever was created.

### 6. Forest Meta is Fully Self-Contained

**DECIDED: `.forest-meta.toml` captures all resolved values at creation time.** The global config is only used by `init` (writes it) and `new` (reads it for defaults/templates). All other commands (`rm`, `ls`, `status`, `exec`) operate solely from the forest's meta file.

This means:
- Changing global config (e.g., `base_branch`) does not retroactively affect existing forests.
- `rm` has everything it needs (source paths, branch names, base branches) without consulting global config.
- No config migration concerns — each forest is a snapshot of its creation-time state.

Example `.forest-meta.toml`:

```toml
name = "review-sues-dialog"
created_at = "2026-02-07T14:30:00Z"
mode = "review"

[[repos]]
name = "foo-api"
source = "/Users/dliv/src/foo-api"
branch = "forest/review-sues-dialog"
base_branch = "dev"
branch_created = true

[[repos]]
name = "foo-web"
source = "/Users/dliv/src/foo-web"
branch = "sue/gh-100/fix-dialog"
base_branch = "dev"
branch_created = false

[[repos]]
name = "foo-infra"
source = "/Users/dliv/src/foo-infra"
branch = "forest/review-sues-dialog"
base_branch = "dev"
branch_created = true

[[repos]]
name = "dev-docs"
source = "/Users/dliv/src/dev-docs"
branch = "forest/review-sues-dialog"
base_branch = "main"
branch_created = true

[[repos]]
name = "bar-workspace"
source = "/Users/dliv/src/bar-workspace"
branch = "forest/review-sues-dialog"
base_branch = "main"
branch_created = true
```

---

## Cross-Cutting Design Principles

These principles apply to all commands and phases. They were established during Phase 2 planning but govern the entire project.

### 7. Agent-Drivable First

The primary consumer of `git-forest` is a software agent (MCP tool, AI coding assistant, shell script). Human UX is important but secondary — the interactive wizard is a convenience layer, not the core interface.

This means:
- **All inputs expressible as flags.** No command requires interactive prompts to function. Interactive features (dialoguer wizard) are deferred to Phase 5 and only activate when stdin is a TTY and required flags are missing.
- **Structured output via `--json`.** Every command supports `--json` for machine-readable output. Human-readable tables are the default. Both are backed by the same data.
- **Actionable error messages.** Every error includes a hint about what to do next, so an agent can parse and recover.
- **Idempotent where possible.** `--force` flags for operations that might conflict. Same flags = same result.
- **No hidden state.** Config and meta files are the only artifacts. Paths are discoverable (`--show-path`).
- **Predictable exit codes.** 0 = success, 1 = user/input error, 2 = system error.

### 8. Commands Return Data, Don't Print

Command functions return typed result structs, not `Result<()>` with `println!` inside. `main.rs` handles all output — either human-readable or JSON based on `--json`.

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
```

This gives us:
- **Testability:** assert on data, not captured stdout.
- **Dual output for free:** human and JSON from the same result structs.
- **Clean boundary:** command logic is pure-ish, IO is at the edges.
- **Future-proof:** an MCP tool or library consumer calls the same functions and gets data back.

This is the practical version of "functional core, imperative shell." The call graph is shallow (main → command → helpers), so there's no deep plumbing problem. IO naturally lives at the two edges (input at the top, output at the bottom) with a thin layer of logic in between. No traits, no DI, no ports-and-adapters machinery needed.

### 9. Plan/Execute for Mutating Commands

Read-only commands (`ls`, `status`) are straightforward: take data, return data. Mutating commands (`init`, `new`, `rm`) use a **plan/execute** split:

1. A **pure planning function** takes inputs and returns a data structure describing what should happen.
2. An **execution function** carries out the plan (filesystem writes, git operations).

For example, `new` will work as:

```rust
plan_forest(inputs) -> Result<Vec<RepoAction>>     // pure: decide what to do
execute_plan(actions) -> Result<NewResult>           // impure: do it
```

The `RepoAction` enum describes operations as data (the command pattern, expressed naturally as a Rust enum):

```rust
enum RepoAction {
    FetchRemote { repo: PathBuf, remote: String },
    CreateWorktree { source: PathBuf, dest: PathBuf, branch: String },
    CreateBranch { repo: PathBuf, branch: String, base: String },
}
```

Benefits:
- **Testable:** assert on the plan without touching git or the filesystem.
- **`--dry-run` for free:** print the actions instead of executing them.
- **Good error reporting:** "failed on step 3 of 7: CreateWorktree { ... }".
- **Agent-inspectable:** `--json --dry-run` lets an agent review the plan before approving execution.

### 10. Debug Assertions for Invariants

Use `debug_assert!` in production code to document and enforce postconditions — invariants that should be guaranteed by the code but would cause subtle bugs downstream if violated. These fire in debug/test builds and compile away in release.

Use `debug_assert!` for **"the code has a bug"** conditions:
- After tilde expansion, paths should be absolute.
- After name derivation, repo names should be non-empty and unique.
- After sanitization, forest names should not contain `/`.
- After planning, action lists should reference valid paths from the input config.

Use proper errors (`bail!`, `Result`) for **"the user gave bad input"** conditions:
- Config file is malformed.
- Repo path doesn't exist.
- Forest name collides with an existing directory.

Place `debug_assert!` at function boundaries — postconditions at the end of functions that produce validated data, preconditions at the start of functions that assume validated input.

---

## Open Questions (Deferred to Implementation Phase)

These are important but don't block starting. Resolve when building the relevant feature.

### `new` command

- **Branch resolution edge cases:**
  - What if a branch is already checked out in another worktree? (Git will error; we need a clear message saying *where*.)
  - What if the user enters a full ref like `origin/foo`? Normalize to short name or reject?
  - Use `git show-ref --verify` for unambiguous local/remote branch checks instead of `rev-parse`.

- **"Review mode" vs "feature mode" — DECIDED (see below).**

- **Upstream tracking:**
  - When creating a new branch off `origin/main`, should we set upstream? Git won't by default for new branches. Document expectations.

### `rm` command

- **Branch deletion safety:**
  - Current spec uses `git branch -D` (force delete) for branches where `branch_created = true`. This can destroy unmerged work.
  - Option: Default to `git branch -d` (safe delete, fails if unmerged). Add `--force` flag for `-D`.
  - Option: Check `git merge-base --is-ancestor` before deleting.
  - Option: Always prompt per-branch before deleting.
  - Add `--dry-run` flag to preview what would be deleted.

- **Dirty worktree handling:**
  - `git worktree remove` fails on uncommitted changes unless `--force` is passed.
  - Should `rm` detect and warn, or just let git's error propagate?

- **Store `base_branch` in meta — DECIDED** (see Decision 6; meta is self-contained, `base_branch` is stored per repo).

### `init` command

- **Re-running init: DECIDED — replace with `--force`, not merge.** Merging is complex and error-prone. For adding repos later, edit the config file or (eventually) `git forest config add-repo`.

### `forest/{name}` branches

- **Naming convention for auto-created base-tracking branches:**
  - In review mode, repos not being reviewed get a `forest/{forest-name}` branch off their base branch.
  - These are marked `branch_created = true` in meta and cleaned up by `rm`.
  - Open: should the branch name include the repo name too (e.g., `forest/{forest-name}/{repo-name}`) or is `forest/{forest-name}` sufficient? Since branch names are per-repo in git, the simpler form should be fine.

---

## Phased Build Plan

### Phase 0 — Skeleton & Foundation (COMPLETE)

**Goal:** Runnable CLI with shared infrastructure. `git forest --help` works.

- Create Rust crate with `clap` derive subcommands (all stubbed).
- Implement config structs + `load_config()` with tilde expansion and validation.
- Implement `.forest-meta.toml` structs + read/write helpers.
- Implement `git()` wrapper with error type.
- Implement path sanitization + forest directory resolution.
- Implement forest discovery (scan `worktree_base` for meta files).

**No git mutations. No interactive prompts.**

### Phase 1 — Read-Only Commands (COMPLETE)

**Goal:** Validate contracts by reading existing forests (manually created or from Phase 3).

- `git forest ls` — scan and list forests with name, age, branch summary.
- `git forest status [name]` — run `git status -sb` per repo in a forest.
- `git forest exec <name> -- <cmd>` — run arbitrary command per repo.

These are simple, low-risk, and exercise config loading, meta parsing, discovery, and the git wrapper.

### Phase 2 — `init` + Output Architecture (COMPLETE)

**Goal:** Non-interactive config generation + refactor all commands for structured output.

See [PHASE_2_PLAN.md](PHASE_2_PLAN.md) for full implementation details.

- All commands (`ls`, `status`, `exec`) return typed result structs (Decision 8). `--json` global flag.
- `init` command: flag-driven, non-interactive config generation with validation and atomic write.
- `debug_assert!` postconditions on key functions (Decision 10).
- 75 tests (63 unit + 12 integration).

### Phase 3 — `new <name>`

**Goal:** Create forests. Start with happy path, layer in complexity.

**DECIDED: interactive flow uses "mode + defaults + exceptions" pattern:**

1. Ask mode once: **"Feature work or PR review?"**
2. Set defaults from mode:
   - Feature → suggested branch (`{user}/{name}`) for all repos (each off its own `base_branch`).
   - Review → `forest/{forest-name}` branch off each repo's `base_branch`.
3. Confirm defaults: "Use `dliv/java-84/refactor-auth` for all repos? (Y/n)"
4. Ask for exceptions via multi-select: **"Which repos should differ from the default?"**
5. Only prompt branch details for the exceptions.

**Typical PR review (most common case):** mode → review, exception → foo-web, enter PR branch. ~3 prompts total.

**Cross-cutting feature:** mode → feature, confirm default, no exceptions. ~2 prompts total.

Uses plan/execute pattern (Decision 9): `plan_forest()` returns `Vec<RepoAction>`, a separate function executes them. `--dry-run` support.

**3a — Minimal happy path (flag-driven, no prompts):**
- Create forest directory.
- Every repo: worktree add with suggested branch.
- Write meta incrementally.

**3b — Mode + exceptions flow (flag-driven or interactive):**
- `git fetch --all` for all repos.
- Mode + defaults + exceptions, expressible as flags or interactive prompts.
- Full branch resolution logic (local → remote → new).

**3c — Polish:**
- Better error messages for common failures (branch exists in another worktree, etc.).

### Phase 4 — `rm <name>` (COMPLETE)

**Goal:** Clean up forests safely.

- Read meta, best-effort cleanup.
- Worktree remove, branch delete (safe by default).
- Handle partial forests gracefully.
- Uses plan/execute pattern (Decision 9): `--dry-run` to preview what would be deleted.
- `--force` flag for destructive operations.
- 138 tests (113 unit + 25 integration).

### Phase 5 — Multi-Template Config

**Goal:** Support multiple repo groups ("templates") so a developer can manage more than one faux-monorepo on a single machine.

Currently the config is a singleton — one set of repos, one `worktree_base`. This is like git only allowing one repo per machine. A developer with opencop-* repos and a separate product's repos needs separate templates.

- Named templates (e.g., `[template.opencop]`, `[template.acme]`), each with its own repos and defaults.
- Default template: when `--template` is omitted, use the default (e.g., `default_template = "opencop"`).
- `git forest new <name> --template acme` to create a forest from a non-default template.
- `git forest init` evolves to create/update templates within the config.
- Backward compatible: existing single-template configs continue to work (treated as the default template).
- `git forest config` subcommand for editing config (add-repo, remove-repo, set defaults).
- **Simplify branch config:** Remove `--username` and `--branch-template`. Replace with `--feature-branch-template` where the user bakes their identity directly into the template (e.g., `--feature-branch-template "dliv/{name}"`). Current `--username` is hidden state that only exists to fill `{user}` in the branch template — the template should be self-contained. Review mode branches (`forest/{name}`) remain hardcoded and don't need a template.

### Phase 6 — Hardening

**Goal:** Polish the agent-friendly core for reliable daily use.

- Accept both original and sanitized names everywhere.
- Auto-detect current forest when inside one (for `rm`, already works for `status`).
- `git forest path <name>` — print forest path for shell integration.
- Improve error messages and edge case handling.
- **Bug: `rm` partial failure leaves orphaned directory.** When `rm` without `--force` fails on a worktree, it still removes the meta file, so a follow-up `rm --force` can't find the forest. Fix: don't remove meta until all worktrees are handled, or re-write a partial meta with remaining repos. Add a test: `rm` partial failure then `rm --force` should recover.
- `--verbose` flag for debugging.
- `--yes` flag for non-interactive use (skip confirmations).
- Detect dirty worktrees and warn before `rm`.
- Tab completion.

### Phase 7 — Interactive UX (optional)

**Goal:** Human-friendly interactive layer on top of the agent-friendly core. Evaluate whether `dialoguer` prompts or a TUI (e.g., `ratatui`) is the right approach.

- Interactive wizard for `init` — guided repo discovery and template setup.
- Interactive prompts for `new` — mode + defaults + exceptions flow (see Phase 3 design).
- Confirmation prompt before `rm` (unless `--yes`).
- Consider TUI for operations like `status` (live-updating multi-repo dashboard) and `new` (interactive repo/branch picker).
- Decision: dialoguer (simple inline prompts) vs. ratatui TUI (richer but heavier) — decide based on which operations benefit from spatial layout vs. simple Q&A.

---

## Future Ideas (Post-v1)

Captured from the original spec plus review discussion:

- `git forest cd <name>` / `git forest switch <name>` — shell/editor integration.
- Hook into `just` — auto-detect justfile and expose recipes.
- `git forest edit` — modify an existing forest's branches.
- Parallel execution in `exec`.
- Multi-remote branch discovery.
- Config migrations / schema versioning.
- MCP tool integration.
