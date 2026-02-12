# git-forest — Architecture Decisions & Project Plan

This document captures architectural decisions, open questions, and the phased build plan for `git-forest`. The original spec lives in [claude-web-init.md](claude-web-init.md).

---

## Decisions to Lock Down Before Coding

These are shared contracts that multiple commands depend on. Changing them later causes rework.

### 1. Config Schema

The config lives at `~/.config/git-forest/config.toml` (XDG).

**Decision needed: field naming consistency.**

The original spec uses `base_branch` in `[general]` but `branch_base` on repo overrides. Pick one name and use it everywhere.

- **Recommendation:** Use `base_branch` globally and per-repo.

**Decision needed: must `name` equal the directory basename of `path`?**

If `path = "~/src/foo-api"`, must `name = "foo-api"`? If so, `name` is redundant and could be derived. If not, we need a separate `dir_name` field or clear documentation that `name` controls the folder name in forests.

- **Recommendation:** Derive `name` from the last segment of `path` by default. Allow explicit `name` as an override for cases where the directory name isn't what you want in the forest.

**Decision needed: per-repo `remote` field.**

The spec assumes `origin` everywhere. Some workflows involve multiple remotes (e.g., fork-based PRs).

- **Recommendation:** Add optional `remote = "origin"` per repo, defaulting to `"origin"`. Defer multi-remote discovery to post-v1.

### 2. Path Handling

**Decision needed: tilde expansion.**

Rust does not expand `~` automatically. The tool must handle this since the config examples use `~`.

- **Recommendation:** Expand `~` to `$HOME` when loading config. Do not support arbitrary env vars. Document this behavior.

**Decision needed: sanitization function for forest directory names.**

Forest names may contain `/` (e.g., `java-84/refactor-auth`). The filesystem directory replaces `/` with `-`.

- **Open question:** `a/b` and `a-b` collide. Options:
  - (A) Detect collision at creation time and error.
  - (B) Append a short hash suffix when collision is detected.
  - (C) Disallow `/` in forest names entirely.
- **Recommendation:** Option A (detect and error) is simplest for v1. Users can pick a different name.

### 3. Forest Identity & Lookup

Commands accept a `<name>` argument. The original name (`java-84/refactor-auth`) and the sanitized directory name (`java-84-refactor-auth`) may differ.

- **Recommendation:** Accept either. Resolve by scanning `.forest-meta.toml` files in `worktree_base`. Match against both the meta `name` field and the directory name.

**Auto-detection:** If the user is inside a forest directory and omits `<name>`, should the tool detect the current forest by walking up to `.forest-meta.toml`?

- **Recommendation:** Yes, for `status`, `exec`, and `rm`. Not for `new` (would be confusing).

### 4. Git Wrapper & Error Model

All git operations go through a helper function. Two modes:

- **Capture:** `git(repo, args) -> Result<String>` — for commands where we need the output (branch checks, status).
- **Stream:** `git_stream(repo, args) -> Result<ExitStatus>` — for commands where output should pass through to the user (exec, fetch).

**Error type** should include: command, args, working directory, exit code, stderr.

**Continue-on-error policy:**
- `exec`: continue to next repo on failure, report non-zero exit at the end.
- `new`: stop on failure, leave partial forest (meta already written incrementally).
- `rm`: best-effort cleanup, continue on individual failures, report all errors at the end.
- `status`: continue on failure (a repo dir might be missing).

### 5. Partial Failure in `new`

If `new` fails midway (network error, branch conflict, etc.), the forest is partially created.

- **Recommendation:** Write `.forest-meta.toml` incrementally — start with the forest header, append each repo entry as it's successfully created. This way `rm` can always clean up whatever was created.

---

## Open Questions (Deferred to Implementation Phase)

These are important but don't block starting. Resolve when building the relevant feature.

### `new` command

- **Branch resolution edge cases:**
  - What if a branch is already checked out in another worktree? (Git will error; we need a clear message saying *where*.)
  - What if the user enters a full ref like `origin/foo`? Normalize to short name or reject?
  - Use `git show-ref --verify` for unambiguous local/remote branch checks instead of `rev-parse`.

- **"Review mode" vs "feature mode" for `branch-on-main` repos:**
  - Current spec infers mode from whether user picked base branch for all mutable repos. This is fragile.
  - Option: Ask once at the start — "Feature or review?" — and use that to set defaults for all repos.
  - Option: Always prompt per-repo (simpler to implement, more verbose).

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

- **Store `base_branch` in meta:**
  - To know "is this branch merged?", `rm` needs to know what the base branch was. Store it in `.forest-meta.toml` at creation time.

### `init` command

- **Re-running init:**
  - Overwrite with confirmation, or merge with existing config?
  - Should it detect existing repos and suggest them?

### Readonly repos

- **Clone source:**
  - `git clone --depth=1 <local-path>` can behave unexpectedly (hardlinks, ignoring depth).
  - Option: Use `--no-local` flag.
  - Option: Clone from the repo's origin URL instead of the local path.
  - Defer decision until implementing readonly clone logic.

---

## Phased Build Plan

### Phase 0 — Skeleton & Foundation

**Goal:** Runnable CLI with shared infrastructure. `git forest --help` works.

- Create Rust crate with `clap` derive subcommands (all stubbed).
- Implement config structs + `load_config()` with tilde expansion and validation.
- Implement `.forest-meta.toml` structs + read/write helpers.
- Implement `git()` wrapper with error type.
- Implement path sanitization + forest directory resolution.
- Implement forest discovery (scan `worktree_base` for meta files).

**No git mutations. No interactive prompts.**

### Phase 1 — Read-Only Commands

**Goal:** Validate contracts by reading existing forests (manually created or from Phase 3).

- `git forest ls` — scan and list forests with name, age, branch summary.
- `git forest status [name]` — run `git status -sb` per repo in a forest.
- `git forest exec <name> -- <cmd>` — run arbitrary command per repo.

These are simple, low-risk, and exercise config loading, meta parsing, discovery, and the git wrapper.

### Phase 2 — `init`

**Goal:** Interactive config generation.

- Implement `dialoguer`-based wizard.
- Write config atomically (write to temp file, then rename).
- Handle re-run with confirmation prompt.

Can start minimal (fewer prompts, sensible defaults) and iterate.

### Phase 3 — `new <name>`

**Goal:** Create forests. Start with happy path, layer in complexity.

**3a — Minimal happy path:**
- Create forest directory.
- Readonly repos: shallow clone.
- Mutable + branch-on-main: worktree add with suggested branch (no prompts).
- Write meta incrementally.

**3b — Branch resolution + prompts:**
- `git fetch --all` for mutable repos.
- Per-repo branch selection prompt (3 options).
- Full branch resolution logic (local → remote → new).

**3c — Polish:**
- Review-mode inference or explicit mode prompt.
- Better error messages for common failures (branch exists in another worktree, etc.).

### Phase 4 — `rm <name>`

**Goal:** Clean up forests safely.

- Read meta, best-effort cleanup.
- Worktree remove, branch delete (safe by default), shallow clone removal.
- Handle partial forests gracefully.
- Confirmation prompt before executing.
- Add `--dry-run` and `--force` flags.

### Phase 5 — Hardening & UX

**Goal:** Polish for daily use.

- Accept both original and sanitized names everywhere.
- Auto-detect current forest when inside one.
- `git forest path <name>` — print forest path for shell integration.
- Improve error messages and edge case handling.
- `--yes` flag for non-interactive use.
- `--verbose` flag for debugging.

---

## Future Ideas (Post-v1)

Captured from the original spec plus review discussion:

- `git forest cd <name>` / `git forest switch <name>` — shell/editor integration.
- Detect dirty worktrees and warn before `rm`.
- Hook into `just` — auto-detect justfile and expose recipes.
- Tab completion.
- `git forest edit` — modify an existing forest's branches.
- Parallel execution in `exec`.
- Multi-remote branch discovery.
- Config migrations / schema versioning.
