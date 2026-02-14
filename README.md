# git-forest

Multi-repo worktree orchestrator. Manages collections of git worktrees ("forests") across multiple repositories for parallel development and PR review workflows.

Installs as `git-forest`, invoked as `git forest <command>`.

## Status

Phase 0 — foundation types and test harness. All subcommands are stubbed.

## Setup

Requires [just](https://just.systems/man/en/).

```
just setup    # configures git hooks
```

## Build & Test

```
just build
just test
just check    # fmt + clippy
```

## Usage

```
git-forest --help
git-forest init          # not yet implemented
git-forest new <name>    # not yet implemented
git-forest rm [name]     # not yet implemented
git-forest ls            # not yet implemented
git-forest status [name] # not yet implemented
git-forest exec <name> -- <cmd>  # not yet implemented
```
