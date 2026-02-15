# git-forest

Multi-repo worktree orchestrator. Manages collections of git worktrees ("forests") across multiple repositories for parallel development and PR review workflows.

If your repos *should* be a monorepo but aren't, this tool makes worktrees across all of them feel like one.

Installs as `git-forest`, invoked as `git forest <command>`.

## Status

Phase 5B complete. Multi-template config support. All core commands work: `init`, `new`, `rm`, `ls`, `status`, `exec`. 160+ tests.

## Quick Start

```sh
# Build
just build

# Configure repos and worktree base
git forest init \
  --feature-branch-template "dliv/{name}" \
  --repo ~/src/foo-api \
  --repo ~/src/foo-web \
  --repo ~/src/foo-infra \
  --repo ~/src/dev-docs \
  --base-branch dev \
  --repo-base-branch dev-docs=main \
  --worktree-base ~/worktrees

# Create a forest for feature work
git forest new my-feature --mode feature

# See what you've got
git forest ls
git forest status my-feature

# Run a command across all repos
git forest exec my-feature -- git log --oneline -3

# Clean up when done
git forest rm my-feature
```

## Commands

```
git forest init     Configure repos and defaults
git forest new      Create a forest (worktrees + branches across all repos)
git forest rm       Remove a forest (worktrees, branches, directory)
git forest ls       List all forests
git forest status   Show git status per repo in a forest
git forest exec     Run a command in each repo of a forest
```

All commands support `--json` for structured output.

### `init`

```
git forest init --feature-branch-template <tmpl> --repo <path> [--repo <path>...] [options]

Options:
  --template <name>                   Template name to create or update (default: default)
  --worktree-base <path>              Base directory for forests (default: ~/worktrees)
  --base-branch <branch>              Default base branch (default: dev)
  --feature-branch-template <tmpl>    Feature branch naming template (must contain {name})
  --repo-base-branch <repo=branch>    Per-repo base branch override (repeatable)
  --force                             Overwrite existing template by the same name
  --show-path                         Print config path and exit
```

### `new`

```
git forest new <name> --mode <feature|review> [options]

Options:
  --template <name>                   Template to use (default: from config)
  --branch <branch>                   Override branch for all repos
  --repo-branch <repo=branch>         Per-repo branch override (repeatable)
  --no-fetch                          Skip fetching remotes
  --dry-run                           Show plan without executing
```

**Feature mode:** All repos get a branch from the feature branch template (e.g., `dliv/{name}`) off their base branch.

**Review mode:** All repos get `forest/{name}` branch. Use `--repo-branch` to point specific repos at a PR branch.

### `rm`

```
git forest rm [name] [options]

Options:
  --force       Force removal of dirty worktrees and unmerged branches
  --dry-run     Show what would be removed without executing
```

Best-effort cleanup: removes worktrees, deletes branches we created, removes the forest directory. Continues on individual failures and reports all errors.

### `ls`, `status`, `exec`

```
git forest ls
git forest status [name]
git forest exec <name> -- <cmd> [args...]
```

`status` and `rm` auto-detect the current forest when run from inside one.

## Development

Requires [just](https://just.systems/man/en/) and [tokei](https://github.com/XAMPPRocky/tokei) (for `just loc`).

```
just setup    # configure git hooks
just build    # build
just test     # run all tests
just check    # fmt --check + clippy
just loc      # count lines of code
```

## Design

See [docs/decisions/](docs/decisions/) for architecture decision records (ADRs). Key principles:

- **Agent-drivable first** ([ADR 0001](docs/decisions/0001-agent-drivable-first.md)) — all inputs as flags, `--json` on every command
- **Commands return data, don't print** ([ADR 0002](docs/decisions/0002-functional-core-imperative-shell.md)) — typed result structs, formatted at the edge
- **Plan/execute for mutations** ([ADR 0003](docs/decisions/0003-plan-execute-split.md)) — `new` and `rm` use plan/execute split with `--dry-run`
- **Best-effort cleanup** ([ADR 0009](docs/decisions/0009-best-effort-error-accumulation.md)) — `rm` continues on failures, accumulates and reports errors

Historical planning docs are in [docs/archive/](docs/archive/).
