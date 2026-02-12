# git-forest — Multi-Repo Worktree Orchestrator

## Overview

`git-forest` is a Rust CLI tool that manages collections of git worktrees (“forests”) across multiple repositories, enabling parallel development and PR review workflows. It installs as `git-forest` so it can be invoked as `git forest <command>`.

The motivating use case: a fullstack developer working across multiple repos (API, web, infra) who needs multiple physical checkouts active simultaneously — e.g., reviewing a PR in one forest while an AI agent works on a feature in another.

## Directory Structure

The user’s source directory contains sibling repos:

```
~/src/
  dev-docs/       # personal agentic context, version controlled, branching
  foo-api/        # company repo, mutable
  foo-web/        # company repo, mutable
  foo-infra/      # company repo, mutable
  bar-workspace/  # coworker's repo, read-only reference
```

A forest is a new directory that mirrors this structure using worktrees, clones, and the original folder names (critical for relative path compatibility):

```
~/worktrees/dliv-java-84-refactor-auth/
  dev-docs/       # worktree branched off main
  foo-api/        # worktree on feature or PR branch
  foo-web/        # worktree on feature or PR branch
  foo-infra/      # worktree on feature or PR branch
  bar-workspace/  # shallow read-only clone on main
  .forest-meta.toml
```

Folder names in the forest **must** match the original repo directory names. Relative paths like `../foo-api` in justfiles and markdown links must resolve correctly within the forest.

## Repo Types

The config defines three repo types:

### `mutable`

Company repos that get branched. In feature mode, new branches are created. In PR mode, existing branches are checked out. Always created as git worktrees from the source repo.

### `branch-on-main`

Repos like `dev-docs` where the user actively makes changes during a feature, but doesn’t follow the same branching conventions. A worktree is created branching off `main` with the feature branch name. The user merges this back to main when done.

### `readonly`

Reference repos like a coworker’s workspace. Created as a shallow clone (`--depth=1`) on `main`. Not modified.

## Configuration

Stored at `~/.config/git-forest/config.toml` (XDG Base Directory).

```toml
[general]
worktree_base = "~/worktrees"
base_branch = "dev"             # default for mutable repos
branch_template = "{user}/{name}" # default suggested branch name
username = "dliv"               # used in branch_template

[[repos]]
name = "foo-api"
path = "~/src/foo-api"
type = "mutable"

[[repos]]
name = "foo-web"
path = "~/src/foo-web"
type = "mutable"

[[repos]]
name = "foo-infra"
path = "~/src/foo-infra"
type = "mutable"

[[repos]]
name = "dev-docs"
path = "~/src/dev-docs"
type = "branch-on-main"
branch_base = "main"            # override base_branch for this repo

[[repos]]
name = "bar-workspace"
path = "~/src/bar-workspace"
type = "readonly"
branch_base = "main"
```

### Branch Template

The `branch_template` supports placeholders:

- `{user}` — from `username` in config
- `{name}` — the forest name passed to `git forest new`

Example: with `branch_template = "{user}/{name}"` and `username = "dliv"`, running `git forest new java-84/refactor-auth` would suggest branch `dliv/java-84/refactor-auth`.

## Commands

### `git forest init`

Interactive setup wizard. Prompts for:

1. Worktree base directory (default: `~/worktrees`)
1. Username (default: from `git config user.name` or system user)
1. Branch naming template
1. Default base branch for mutable repos
1. For each repo:
- Name (used as folder name in forests)
- Path to source repo
- Type: `mutable`, `branch-on-main`, or `readonly`
- Base branch override (if different from default)

Writes config to `~/.config/git-forest/config.toml`. Re-running `init` overwrites the config (with confirmation).

### `git forest new <name>`

Creates a new forest. `<name>` is a human identifier (e.g., `java-84/refactor-auth` or `review-bobs-pr`).

**Flow:**

1. Create forest directory at `{worktree_base}/{name}` (sanitize slashes to dashes in directory name, e.g. `java-84/refactor-auth` → `java-84-refactor-auth`)
1. Run `git fetch --all` in each mutable repo
1. For each **mutable** repo, prompt:
   
   ```
   foo-api:
     [1] dev (base branch)
     [2] dliv/java-84/refactor-auth (suggested)
     [3] Enter branch name
   Choice [2]:
   ```
- Option 1: Creates worktree on `dev` (or configured base branch)
- Option 2: Creates worktree with suggested branch name (new branch off `origin/{base_branch}`)
- Option 3: Prompts for branch name, fetches, checks out (existing remote or local branch) or creates new branch
1. For each **branch-on-main** repo: create worktree branching off `main` with the suggested feature branch name (for feature work) or on `main` directly (if user selects option 1 for all mutable repos, implying a review)
1. For each **readonly** repo: shallow clone (`git clone --depth=1 --branch main`) into the forest directory
1. Write `.forest-meta.toml` to the forest root

### `.forest-meta.toml`

```toml
name = "java-84/refactor-auth"
created_at = "2026-02-07T14:30:00Z"

[[repos]]
name = "foo-api"
branch = "dliv/java-84/refactor-auth"
branch_created = true    # we created this branch (cleanup should delete it)

[[repos]]
name = "foo-web"
branch = "dliv/java-84/refactor-auth"
branch_created = true

[[repos]]
name = "foo-infra"
branch = "dliv/java-84/refactor-auth"
branch_created = true

[[repos]]
name = "dev-docs"
branch = "dliv/java-84/refactor-auth"
branch_created = true

[[repos]]
name = "bar-workspace"
branch = "main"
branch_created = false
```

The `branch_created` field indicates whether `git forest rm` should delete the local branch after removing the worktree.

### `git forest rm <name>`

Cleanup a forest completely:

1. Read `.forest-meta.toml`
1. For each mutable and branch-on-main repo:
- `git worktree remove <path>`
- If `branch_created = true`: `git branch -D <branch>` in the source repo
1. For each readonly repo: `rm -rf` the shallow clone
1. Remove the forest directory
1. Run `git worktree prune` in each source repo

Prompt for confirmation before executing.

### `git forest ls`

List all forests. Reads `.forest-meta.toml` from each subdirectory of `worktree_base`.

```
NAME                          CREATED     BRANCHES
java-84-refactor-auth         2d ago      dliv/java-84/refactor-auth (api, web, infra)
review-bobs-pr                4h ago      bob/auth-stuff (api), JIRA-999 (infra), dev (web)
```

Format is flexible and can be iterated on. Core info: name, age, branch summary.

### `git forest status [name]`

If `name` provided, show `git status -sb` for each repo in that forest.
If no `name`, show status for all forests (or prompt to select one).

```
=== java-84-refactor-auth ===
  foo-api:   ## dliv/java-84/refactor-auth...origin/dev [ahead 3]
  foo-web:   ## dliv/java-84/refactor-auth...origin/dev
  foo-infra: ## dliv/java-84/refactor-auth...origin/dev [ahead 1]
  dev-docs:  ## dliv/java-84/refactor-auth...origin/main [ahead 2]
```

### `git forest exec <name> -- <command>`

Run an arbitrary command in each repo directory of the named forest.

```bash
$ git forest exec java-84-refactor-auth -- git pull
=== foo-api ===
Already up to date.
=== foo-web ===
Updating 3a4b5c6..7d8e9f0
...
```

Runs in each directory sequentially. Prints repo name before each output. Non-zero exit codes are reported but don’t stop execution of remaining repos.

## Technical Details

### Dependencies

- **clap** — CLI parsing with derive macros. Subcommand per command.
- **dialoguer** — Interactive prompts (Select, Input, Confirm).
- **serde** + **toml** — Config and metadata serialization.
- **chrono** — Timestamps for metadata.
- **directories** — XDG base directory resolution (`config_dir()`).
- **std::process::Command** — All git operations. No `git2`/`libgit2`.

### Git Operations

Wrap git calls in a helper:

```rust
fn git(repo: &Path, args: &[&str]) -> Result<String>
```

Key operations:

- `git fetch --all`
- `git worktree add <path> -b <branch> <start_point>`
- `git worktree add <path> <existing_branch>`
- `git worktree remove <path>`
- `git worktree prune`
- `git branch -D <branch>`
- `git clone --depth=1 --branch <branch> <source> <dest>`
- `git status -sb`
- `git rev-parse --verify <ref>` (check if branch exists locally or on remote)

### Branch Resolution Logic (for `new` command)

When a user provides a branch name for a mutable repo:

```
1. Check if local branch exists → worktree add <path> <branch>
2. Check if origin/<branch> exists → worktree add <path> -b <branch> origin/<branch>
3. Neither exists → worktree add <path> -b <branch> origin/<base_branch>
```

This handles: own new features (case 3), coworker PR branches (case 2), and resuming own work (case 1).

### Directory Name Sanitization

Forest names may contain slashes (e.g., `java-84/refactor-auth`). The filesystem directory name should replace `/` with `-` to avoid nested directories. The original name is preserved in `.forest-meta.toml`.

### Error Handling

- If a worktree already exists for a branch, git will error. Detect and report clearly.
- If `git forest rm` is run on a partially-created forest (e.g., after a failure during `new`), handle missing worktrees/directories gracefully.
- Validate config exists before running any command other than `init`. Print helpful message pointing to `git forest init`.

## Platform

macOS. No Linux/Windows considerations needed for v1.

## Out of Scope (v1)

- Remote operations (push, pull within forests)
- Automatic conflict detection across repos
- Integration with GitHub/GitLab PR APIs
- Parallel command execution in `exec`
- Tab completion (nice to have later)
- `git forest edit` to modify an existing forest’s branches

## Future Ideas

- `git forest cd <name>` — print the forest path for shell integration (`cd $(git forest cd foo)`)
- `git forest switch <name>` — open the forest directory in the user’s editor/IDE
- Detect dirty worktrees and warn before `rm`
- Hook into `just` — auto-detect justfile in the forest and expose common recipes
